/** Only same-origin app paths may be used as a post-sign-in destination (no open redirects). */
export function safeNext(next: string): string {
  return next.startsWith('/') && !next.startsWith('//') && !next.startsWith('/\\') ? next : '/'
}
