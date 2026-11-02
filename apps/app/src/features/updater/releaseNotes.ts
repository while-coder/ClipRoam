export function releaseNotesForVersion(changelog: string, version: string): string {
  const lines = changelog.split(/\r?\n/)
  const start = lines.findIndex(line => line.trim() === `## ${version.replace(/^v/, '')}`)
  if (start < 0) return ''
  const end = lines.findIndex((line, index) => index > start && /^##\s/.test(line))
  return lines.slice(start + 1, end < 0 ? undefined : end).join('\n').trim()
}
