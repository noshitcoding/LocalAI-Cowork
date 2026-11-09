/** A suggested URL only; mounting the view must never navigate implicitly. */
export function defaultBrowserUrl(development = import.meta.env.DEV): string {
  return development ? 'http://127.0.0.1:5173' : 'https://example.com'
}
