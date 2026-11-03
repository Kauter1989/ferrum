#!/usr/bin/env node
/**
 * Load `lib/client.js` against a stub `window.__ModuleLoader__` and check the
 * registrations it makes (criterion 25.6-b, the offline half).
 *
 *   node integrations/dsh/plugin/smoke-load.cjs
 *
 * The harness never runs the bundle before it serves it, so this is the only
 * check of the wrapper, the package id and the slot registrations that needs
 * no harness.
 */
"use strict";

const fs = require("fs");
const path = require("path");
const vm = require("vm");

const packageJson = JSON.parse(fs.readFileSync(path.join(__dirname, "package.json"), "utf8"));
const bundle = fs.readFileSync(path.join(__dirname, "lib", "client.js"), "utf8");

function fail(message) {
	console.error("smoke-load: " + message);
	process.exit(1);
}

function assert(condition, message) {
	if (!condition) fail(message);
}

// The loader facade the harness injects before any bundle runs.
let captured = null;
const sandbox = {
	window: {
		__ModuleLoader__: {
			load(entry) {
				if (captured) fail("the bundle registered more than once");
				captured = entry;
			}
		},
		localStorage: {
			getItem: () => null,
			setItem: () => {}
		}
	}
};
sandbox.globalThis = sandbox;
vm.runInNewContext(bundle, sandbox, { filename: "lib/client.js" });

assert(
	packageJson.dsh && packageJson.dsh.client && packageJson.dsh.client.platform === "web",
	"package.json does not declare dsh.client.platform = web"
);
assert(packageJson.exports && packageJson.exports["./client"], "package.json has no ./client export");
assert(captured, "the bundle never called window.__ModuleLoader__.load");

const id = captured.id;
assert(id === packageJson.name, "the bundle id " + JSON.stringify(id) + " is not the package name");
assert(typeof captured.factory === "function", "the entry has no factory");

// A stub React: nothing is rendered here, only the registrations are checked.
let renders = 0;
const React = {
	createElement: () => {
		renders += 1;
		return null;
	},
	useState: (initial) => [initial, () => {}],
	useEffect: () => {}
};
const requireStub = (name) => {
	assert(name === "react", "the bundle required " + JSON.stringify(name) + ", which is not a platform seed");
	return React;
};

const plugin = captured.factory(requireStub);
assert(typeof plugin.apply === "function", "the exports have no apply()");
assert(Array.isArray(plugin.inject), "the exports have no inject array");
assert(plugin.inject.includes("slots"), "the plugin does not inject the slots service");

const registrations = [];
const slots = {
	inject(name, register) {
		registrations.push(register());
	},
	register(options) {
		return { slot: options.name, options };
	}
};
const layout = { selectPanel: (panel) => registrations.push({ selectPanel: panel }) };
plugin.apply({ slots, layout });

assert(registrations.length >= 3, "apply() made " + registrations.length + " registrations, expected three");
const bySlot = (name) => registrations.find((registered) => registered.slot === name);
assert(bySlot("main"), "no main page registration");
assert(bySlot("main").options.key === plugin.PANEL_ID, "the main page has no stable key");
assert(bySlot("sidebar.panellist"), "no sidebar row registration");
assert(bySlot("sidebar.panellist").options.label === "FERRUM", "the sidebar row is not labelled FERRUM");
assert(bySlot("conversation.session.header.actions"), "no conversation header button");
assert(renders === 0, "loading the bundle rendered something");

// The header button opens the page through the layout service.
bySlot("conversation.session.header.actions").options.inject().open();
assert(
	registrations.some((entry) => entry.selectPanel === plugin.PANEL_ID),
	"the header button does not select the FERRUM page"
);

// The two guards the page relies on: loopback only, and a 32-hex-digit token.
assert(plugin.isLoopback("http://127.0.0.1:41234"), "a loopback address was refused");
assert(plugin.isLoopback("http://localhost:8765"), "localhost was refused");
assert(!plugin.isLoopback("https://example.com"), "a remote address was accepted");
assert(!plugin.isLoopback("http://127.0.0.1:41234/evil"), "a path behind the origin was accepted");
assert(plugin.tokenLooksValid("0eae1ca5bede53331c1e148bb2be1e68"), "a 32-hex token was refused");
assert(!plugin.tokenLooksValid("short"), "a short token was accepted");
assert(!plugin.tokenLooksValid("0eae1ca5bede53331c1e148bb2be1e6g"), "a non-hex token was accepted");

const slotsRegistered = registrations.filter((entry) => entry.slot).length;
console.log("smoke-load: ok — " + id + " registers " + slotsRegistered + " slots");
