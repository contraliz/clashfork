//! Read-only best-route advisory. This never changes adapters or routing.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunRouteReadiness {
    Ready,
    CompetingTunnel,
    Pending,
    Indeterminate,
    #[cfg_attr(windows, allow(dead_code))]
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TunRouteProbe {
    pub readiness: TunRouteReadiness,
    /// `Some(true)` means a public destination currently selects ClashforkTUN.
    /// `None` means the best-route API could not be queried reliably.
    pub has_owned_route: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteObservation {
    Clashfork,
    OtherTunnel,
    OtherInterface,
    Unknown,
}

fn classify_observations(routes: [RouteObservation; 2]) -> TunRouteReadiness {
    if routes.contains(&RouteObservation::OtherTunnel) {
        TunRouteReadiness::CompetingTunnel
    } else if routes
        .iter()
        .all(|route| *route == RouteObservation::Clashfork)
    {
        TunRouteReadiness::Ready
    } else if routes.contains(&RouteObservation::Unknown) {
        TunRouteReadiness::Indeterminate
    } else {
        TunRouteReadiness::Pending
    }
}

fn owned_route_present(routes: [RouteObservation; 2]) -> Option<bool> {
    if routes.contains(&RouteObservation::Unknown) {
        None
    } else {
        Some(routes.contains(&RouteObservation::Clashfork))
    }
}

/// Reads the Windows best routes for two public IPv4 addresses and checks whether both have
/// settled onto ClashforkTUN. This is observational only; it sends no packets and never changes
/// interface or route settings.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn probe_tun_routes() -> TunRouteProbe {
    use windows_sys::Win32::{
        NetworkManagement::{
            IpHelper::{GetBestInterfaceEx, GetIfEntry2, IF_TYPE_TUNNEL, MIB_IF_ROW2},
            Ndis::{IfOperStatusUp, TUNNEL_TYPE_NONE},
        },
        Networking::WinSock::{AF_INET, IN_ADDR, IN_ADDR_0, SOCKADDR, SOCKADDR_IN},
    };

    fn observe(address: [u8; 4]) -> RouteObservation {
        let destination = SOCKADDR_IN {
            sin_family: AF_INET,
            sin_port: 0,
            sin_addr: IN_ADDR {
                S_un: IN_ADDR_0 {
                    S_addr: u32::from_ne_bytes(address),
                },
            },
            sin_zero: [0; 8],
        };
        let mut interface_index = 0u32;
        let route_result = unsafe {
            GetBestInterfaceEx(
                (&destination as *const SOCKADDR_IN).cast::<SOCKADDR>(),
                &mut interface_index,
            )
        };
        if route_result != 0 || interface_index == 0 {
            return RouteObservation::Unknown;
        }

        let mut interface = MIB_IF_ROW2 {
            InterfaceIndex: interface_index,
            ..MIB_IF_ROW2::default()
        };
        if unsafe { GetIfEntry2(&mut interface) } != 0 {
            return RouteObservation::Unknown;
        }
        let alias = String::from_utf16_lossy(&interface.Alias);
        let description = String::from_utf16_lossy(&interface.Description);
        if interface.OperStatus != IfOperStatusUp {
            return RouteObservation::OtherInterface;
        }
        if alias
            .trim_end_matches('\0')
            .eq_ignore_ascii_case(crate::rules::TUN_DEVICE)
        {
            RouteObservation::Clashfork
        } else if is_tunnel_route(
            true,
            interface.Type == IF_TYPE_TUNNEL || interface.TunnelType != TUNNEL_TYPE_NONE,
            &alias,
            &description,
        ) {
            RouteObservation::OtherTunnel
        } else {
            RouteObservation::OtherInterface
        }
    }

    let routes = [observe([1, 1, 1, 1]), observe([8, 8, 8, 8])];
    TunRouteProbe {
        readiness: classify_observations(routes),
        has_owned_route: owned_route_present(routes),
    }
}

#[cfg(target_os = "macos")]
fn macos_interface_for(address: &str) -> Option<String> {
    let output = std::process::Command::new("/sbin/route")
        .args(["-n", "get", address])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .find_map(|line| {
            let (key, value) = line.trim().split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case("interface")
                .then(|| value.trim().to_owned())
        })
}

#[cfg(target_os = "macos")]
fn classify_macos_routes(routes: [Option<String>; 2]) -> TunRouteProbe {
    let device = crate::rules::TUN_DEVICE;
    let owned = routes
        .iter()
        .flatten()
        .any(|name| name.eq_ignore_ascii_case(device));
    let all_known = routes.iter().all(Option::is_some);
    let both_owned = routes.iter().all(|name| {
        name.as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(device))
    });
    let competing_utun = routes
        .iter()
        .flatten()
        .any(|name| name.starts_with("utun") && !name.eq_ignore_ascii_case(device));
    let readiness = if !all_known {
        TunRouteReadiness::Indeterminate
    } else if both_owned {
        TunRouteReadiness::Ready
    } else if competing_utun {
        TunRouteReadiness::CompetingTunnel
    } else {
        TunRouteReadiness::Pending
    };
    TunRouteProbe {
        readiness,
        has_owned_route: if owned {
            Some(true)
        } else if all_known {
            Some(false)
        } else {
            None
        },
    }
}

#[cfg(target_os = "macos")]
pub fn probe_tun_routes() -> TunRouteProbe {
    classify_macos_routes([
        macos_interface_for("1.1.1.1"),
        macos_interface_for("8.8.8.8"),
    ])
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn probe_tun_routes() -> TunRouteProbe {
    TunRouteProbe {
        readiness: TunRouteReadiness::Unsupported,
        has_owned_route: None,
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
pub fn active_competing_tunnel() -> bool {
    use windows_sys::Win32::{
        NetworkManagement::{
            IpHelper::{GetBestInterfaceEx, GetIfEntry2, IF_TYPE_TUNNEL, MIB_IF_ROW2},
            Ndis::{IfOperStatusUp, TUNNEL_TYPE_NONE},
        },
        Networking::WinSock::{AF_INET, IN_ADDR, IN_ADDR_0, SOCKADDR, SOCKADDR_IN},
    };

    // Route lookup is observational and asks which interface Windows would use
    // for a public IPv4 destination. No packet is sent.
    let destination = SOCKADDR_IN {
        sin_family: AF_INET,
        sin_port: 0,
        sin_addr: IN_ADDR {
            S_un: IN_ADDR_0 {
                S_addr: u32::from_ne_bytes([1, 1, 1, 1]),
            },
        },
        sin_zero: [0; 8],
    };
    let mut interface_index = 0u32;
    let route_result = unsafe {
        GetBestInterfaceEx(
            (&destination as *const SOCKADDR_IN).cast::<SOCKADDR>(),
            &mut interface_index,
        )
    };
    if route_result != 0 || interface_index == 0 {
        return false;
    }

    let mut interface = MIB_IF_ROW2 {
        InterfaceIndex: interface_index,
        ..MIB_IF_ROW2::default()
    };
    if unsafe { GetIfEntry2(&mut interface) } != 0 {
        return false;
    }
    let alias = String::from_utf16_lossy(&interface.Alias);
    let description = String::from_utf16_lossy(&interface.Description);
    is_tunnel_route(
        interface.OperStatus == IfOperStatusUp,
        interface.Type == IF_TYPE_TUNNEL || interface.TunnelType != TUNNEL_TYPE_NONE,
        &alias,
        &description,
    )
}

fn is_tunnel_route(is_up: bool, native_tunnel_type: bool, alias: &str, description: &str) -> bool {
    if !is_up {
        return false;
    }
    if native_tunnel_type {
        return true;
    }
    [alias, description].iter().any(|name| {
        let name = name.trim_end_matches('\0').to_ascii_lowercase();
        ["tunnel", "vpn", "wintun", "wireguard", "tap"]
            .iter()
            .any(|marker| name.contains(marker))
    })
}

#[cfg(not(windows))]
pub fn active_competing_tunnel() -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn route_advisory_requires_an_up_tunnel_style_adapter() {
        assert!(!super::is_tunnel_route(false, true, "VPN tunnel", ""));
        assert!(super::is_tunnel_route(true, true, "", ""));
        assert!(super::is_tunnel_route(true, false, "Fixture VPN", ""));
        assert!(!super::is_tunnel_route(true, false, "Ethernet", ""));
    }

    #[test]
    fn unsupported_platforms_report_no_advisory() {
        #[cfg(not(windows))]
        assert!(!super::active_competing_tunnel());
    }

    #[test]
    fn route_readiness_requires_both_public_routes_on_the_clashfork_adapter() {
        use super::{RouteObservation as Route, TunRouteReadiness as Readiness};

        assert_eq!(
            super::classify_observations([Route::Clashfork, Route::Clashfork]),
            Readiness::Ready
        );
        assert_eq!(
            super::classify_observations([Route::Clashfork, Route::OtherInterface]),
            Readiness::Pending
        );
        assert_eq!(
            super::classify_observations([Route::Clashfork, Route::OtherTunnel]),
            Readiness::CompetingTunnel
        );
        assert_eq!(
            super::classify_observations([Route::Unknown, Route::OtherInterface]),
            Readiness::Indeterminate
        );
        assert_eq!(
            super::owned_route_present([Route::Clashfork, Route::Unknown]),
            None
        );
        assert_eq!(
            super::owned_route_present([Route::OtherTunnel, Route::OtherInterface]),
            Some(false)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_route_readiness_matches_the_configured_utun_device() {
        let device = crate::rules::TUN_DEVICE.to_owned();
        let other_utun = if device == "utun0" { "utun1" } else { "utun0" }.to_owned();
        let direct = "en0".to_owned();
        let owned = super::classify_macos_routes([Some(device.clone()), Some(device.clone())]);
        assert_eq!(owned.readiness, super::TunRouteReadiness::Ready);
        assert_eq!(owned.has_owned_route, Some(true));

        let handoff = super::classify_macos_routes([Some(device), Some(direct.clone())]);
        assert_eq!(handoff.readiness, super::TunRouteReadiness::Pending);
        assert_eq!(handoff.has_owned_route, Some(true));

        let competing = super::classify_macos_routes([Some(other_utun), Some(direct.clone())]);
        assert_eq!(
            competing.readiness,
            super::TunRouteReadiness::CompetingTunnel
        );
        assert_eq!(competing.has_owned_route, Some(false));

        let direct = super::classify_macos_routes([Some(direct), None]);
        assert_eq!(direct.readiness, super::TunRouteReadiness::Indeterminate);
        assert_eq!(direct.has_owned_route, None);
    }
}
