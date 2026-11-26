const MAX_FILE_NAME_CHARS = 120
const RESERVED_NAMES = /^(CON|PRN|AUX|NUL|CLOCK\$|COM[1-9]|LPT[1-9])$/i

export function safeExportFileName(input: string, fallback = 'clashfork-routing'): string {
  const trimmed = input.trim()
  const stem = trimmed.toLowerCase().endsWith('.json') ? trimmed.slice(0, -5) : trimmed
  let name = stem
    .replace(/[\u0000-\u001f<>:"/\\|?*]/g, '-')
    .replace(/^[ .]+|[ .]+$/g, '')
  if (!name || /^[- .]+$/.test(name)) name = fallback
  if (RESERVED_NAMES.test(name)) name = `clashfork-${name}`
  const keep = MAX_FILE_NAME_CHARS - 5
  if (name.length > keep) name = name.slice(0, keep).replace(/[ .]+$/g, '')
  if (!name) name = fallback
  return `${name}.json`
}
