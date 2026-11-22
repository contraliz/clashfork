#![allow(
    unsafe_code,
    reason = "Windows DPAPI and filesystem ACLs require Win32 calls"
)]

use anyhow::{Context, Result, bail};
use clashfork_service_ipc::{OwnerCredentials, OwnerIdentity};
use std::{
    ffi::c_void,
    fs::File,
    io::Write as _,
    os::windows::{
        ffi::OsStrExt as _,
        io::{FromRawHandle as _, RawHandle},
    },
    path::Path,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        },
        Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_PROMPTSTRUCT, CRYPTPROTECT_UI_FORBIDDEN,
            CryptProtectData, CryptUnprotectData,
        },
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    },
    Storage::FileSystem::{
        CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

const TOKEN_BYTES: usize = 32;

pub fn current_owner_credentials(app_data_root: &Path) -> Result<OwnerCredentials> {
    std::fs::create_dir_all(app_data_root)?;
    let root = std::fs::canonicalize(app_data_root)
        .with_context(|| format!("failed to canonicalize {:?}", app_data_root))?;
    let sid = current_sid()?;
    let token = load_or_create_owner_token(&root, &sid)?;
    Ok(OwnerCredentials {
        identity: OwnerIdentity::Windows { sid },
        app_data_dir: root.to_string_lossy().into_owned(),
        token: Some(hex(&token)),
    })
}

fn load_or_create_owner_token(root: &Path, sid: &str) -> Result<[u8; TOKEN_BYTES]> {
    let path = root.join(clashfork_service_ipc::OWNER_TOKEN_FILE_NAME);
    if path.is_file() {
        let mut file = File::open(&path).context("failed to open the owner token")?;
        let mut token = [0u8; TOKEN_BYTES];
        use std::io::Read as _;
        file.read_exact(&mut token)
            .context("the owner token is unreadable; reinstall Clashfork for this user")?;
        return Ok(token);
    }
    let token = random_token();
    let sddl = format!("D:P(A;;FA;;;{sid})(A;;FA;;;SY)(A;;FA;;;BA)");
    create_private_file(&path, &token, &sddl, true)
        .context("failed to create the service owner token")?;
    Ok(token)
}

fn random_token() -> [u8; TOKEN_BYTES] {
    let mut bytes = [0u8; TOKEN_BYTES];
    let first = uuid::Uuid::new_v4();
    let second = uuid::Uuid::new_v4();
    bytes[..16].copy_from_slice(first.as_bytes());
    bytes[16..].copy_from_slice(second.as_bytes());
    bytes
}

pub fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let sid = current_sid()?;
    let sddl = format!("D:P(A;;FA;;;{sid})");
    create_private_file(path, bytes, &sddl, false)
}

pub fn write_secrets_file(path: &Path, plaintext: &[u8]) -> Result<()> {
    let protected = protect(plaintext)?;
    write_private_file(path, &protected)
}

pub fn read_secrets_file(path: &Path) -> Result<Vec<u8>> {
    let protected = std::fs::read(path).context("failed to read the protected credential file")?;
    unprotect(&protected)
}

fn create_private_file(path: &Path, bytes: &[u8], sddl: &str, hidden: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let wide_sddl: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide_sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 || descriptor.is_null() {
        bail!("failed to build a protected file ACL");
    }
    let descriptor = OwnedDescriptor(descriptor);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let wide_path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ,
            &attributes,
            CREATE_ALWAYS,
            FILE_FLAG_OPEN_REPARSE_POINT
                | if hidden {
                    FILE_ATTRIBUTE_HIDDEN
                } else {
                    FILE_ATTRIBUTE_NORMAL
                },
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error()).context("failed to create a protected file");
    }
    let mut file = unsafe { File::from_raw_handle(handle as RawHandle) };
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub fn protect(bytes: &[u8]) -> Result<Vec<u8>> {
    crypt(bytes, true)
}

pub fn unprotect(bytes: &[u8]) -> Result<Vec<u8>> {
    crypt(bytes, false)
}

fn crypt(bytes: &[u8], protect_data: bool) -> Result<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).context("secret payload is too large")?,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let mut entropy_bytes = b"Clashfork.local.secrets.v1".to_vec();
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(entropy_bytes.len()).context("entropy too large")?,
        pbData: entropy_bytes.as_mut_ptr(),
    };
    let ok = unsafe {
        if protect_data {
            CryptProtectData(
                &input,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null::<CRYPTPROTECT_PROMPTSTRUCT>(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null::<CRYPTPROTECT_PROMPTSTRUCT>(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        bail!("DPAPI rejected the local credential operation");
    }
    if output.pbData.is_null() || output.cbData == 0 {
        return Ok(Vec::new());
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut c_void);
    }
    Ok(result)
}

fn current_sid() -> Result<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if ok == 0 {
        bail!("failed to read the current Windows user identity");
    }
    let token = OwnedHandle(token);
    let mut needed = 0u32;
    unsafe {
        GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 {
        bail!("failed to size the current Windows user token");
    }
    let mut buffer = vec![0u8; needed as usize];
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    if ok == 0 {
        bail!("failed to read the current Windows user token");
    }
    let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
    let mut sid_string: *mut u16 = std::ptr::null_mut();
    let ok = unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid_string) };
    if ok == 0 || sid_string.is_null() {
        bail!("failed to format the current Windows user SID");
    }
    let mut len = 0usize;
    unsafe {
        while *sid_string.add(len) != 0 {
            len += 1;
        }
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(sid_string, len) });
    unsafe {
        LocalFree(sid_string as *mut c_void);
    }
    Ok(value)
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0xf) as usize] as char);
    }
    output
}

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

struct OwnedDescriptor(PSECURITY_DESCRIPTOR);
impl Drop for OwnedDescriptor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0 as *mut c_void);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_roundtrips_current_user() {
        let plaintext = b"fixture secret bytes";
        let protected = protect(plaintext).unwrap();
        assert_ne!(protected, plaintext);
        assert_eq!(unprotect(&protected).unwrap(), plaintext);
    }
}
