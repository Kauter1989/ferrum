/**
 * Host half of the FERRUM view plugin.
 *
 * Every client plugin needs one: `dsh-client-modules` serves the
 * `exports["./client"]` bundle only for Loader rows whose package declares
 * `dsh.client`, and a Loader row needs a module with an `apply` to hold its
 * fiber. All the work happens in the browser half (`lib/client.js`).
 *
 * Keep this file import-free: a package mounted by an absolute path or a
 * `file:` URL from the profile patch has no `node_modules` to resolve from.
 */
export function apply() {}
