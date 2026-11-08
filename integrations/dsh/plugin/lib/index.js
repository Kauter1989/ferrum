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

/**
 * Records that the row was activated, so a person can tell "the plugin is
 * loaded but its panel is not visible" from "the row never loaded".
 *
 * Writing is best effort and never throws: a plugin must not fail to apply
 * because a log file could not be opened.
 */
function noteActivation() {
	try {
		const fs = process.getBuiltinModule("node:fs");
		const path = process.getBuiltinModule("node:path");
		const file = path.join(process.cwd(), "target", "ferrum-panel-activation.log");
		fs.appendFileSync(file, `${new Date().toISOString()} pid=${process.pid} client-bundle=${import.meta.url}\n`);
	} catch (e) {
		/* no writable log: the plugin still applies */
	}
}

export function apply() {
	noteActivation();
}