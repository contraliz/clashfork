export function downloadJsonFile(contents: string, fileName: string) {
  const blob = new Blob([contents], { type: 'application/json' })
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = fileName
  anchor.style.display = 'none'
  document.body.append(anchor)
  anchor.click()
  anchor.remove()
  // WebView downloads resolve asynchronously; keep the Blob URL alive long enough
  // for the browser to read it, then release it without racing the download.
  window.setTimeout(() => URL.revokeObjectURL(url), 30_000)
}
