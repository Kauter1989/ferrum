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
// The header button asks the pane which surface has focus.
sandbox.document = { activeElement: null };
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
// The right sidebar's two services, as DSH provides them: a tab-type registry
// and the pane controller whose `openTab` shows a tab.
const tabTypes = [];
const sidebarRightTabs = {
	register(definition) {
		tabTypes.push(definition);
	}
};
const opened = [];
const toggles = [];
let expanded = false;
const sidebarRight = {
	// The pane controller resolves the session from the focused surface, as the
	// built-in Browser does; the header button must use that path.
	commandTarget() {
		return { sessionId: "s1", paneId: "p1", host: "dock" };
	},
	openTabFromTarget(kind, target) {
		opened.push({ kind, target });
	},
	openTab(kind, options) {
		opened.push({ kind, options, wrong: "the header must bind the tab to a session, not open it blindly" });
	},
	isExpanded() {
		return expanded;
	},
	toggleExpanded() {
		expanded = !expanded;
		toggles.push(expanded);
	}
};
plugin.apply({ slots, layout, sidebarRightTabs, sidebarRight });

assert(registrations.length >= 5, "apply() made " + registrations.length + " registrations, expected five");
const bySlot = (name) => registrations.find((registered) => registered.slot === name);
assert(bySlot("main"), "no main page registration");
assert(bySlot("main").options.key === plugin.PANEL_ID, "the main page has no stable key");
assert(bySlot("sidebar.panellist"), "no sidebar row registration");
assert(bySlot("sidebar.panellist").options.label === "FERRUM", "the sidebar row is not labelled FERRUM");
assert(bySlot("conversation.session.header.actions"), "no conversation header button");

// The right sidebar: one tab type and the two slots it needs.
assert(tabTypes.length === 1, "expected one right-sidebar tab type, got " + tabTypes.length);
assert(tabTypes[0].kind === plugin.PANEL_ID, "the tab type kind is not the panel id");
assert(tabTypes[0].multiple === false, "the FERRUM tab should not be openable twice");
// The strip's "+" menu lists these entries: an empty list means the type
// exists but can be opened from nowhere.
assert(Array.isArray(tabTypes[0].guide) && tabTypes[0].guide.length >= 1, "the tab type has no guide entry");
assert(typeof tabTypes[0].guide[0].title === "function", "the guide entry has no title function");
assert(tabTypes[0].guide[0].title() === "FERRUM", "the guide entry is not labelled FERRUM");
assert(bySlot("sidebar.right.pane.tab"), "no right-pane tab registration");
assert(bySlot("sidebar.right.pane.tab").options.key === plugin.PANEL_ID, "the right-pane tab has no stable key");
assert(bySlot("sidebar.right.pane.tab.title"), "no right-pane tab title registration");
assert(renders === 0, "loading the bundle rendered something");

// The header button opens the panel in the right sidebar, next to the chat,
// bound to the session the focused surface belongs to.
bySlot("conversation.session.header.actions").options.inject().open();
assert(opened.length === 1, "the header button did not open a right-sidebar tab");
assert(opened[0].kind === plugin.PANEL_ID, "the header button opened the wrong tab kind");
assert(opened[0].wrong === undefined, "the header opened a tab without binding it to a session");
assert(
	opened[0].target && opened[0].target.sessionId === "s1",
	"the tab was not bound to the focused session"
);
assert(
	!registrations.some((entry) => entry.selectPanel === plugin.PANEL_ID),
	"the header button took over the main column even though a right sidebar exists"
);
assert(toggles.length === 1 && expanded, "the header button did not reveal the collapsed right sidebar");
// A pane that is already open is left alone.
toggles.length = 0;
bySlot("conversation.session.header.actions").options.inject().open();
assert(toggles.length === 0, "the header button collapsed an already open right sidebar");

// A build without the right sidebar keeps the panel working in the centre.
const fallback = [];
plugin.apply({
	slots: {
		inject: (name, register) => fallback.push(register()),
		register: (options) => ({ slot: options.name, options })
	},
	layout: { selectPanel: (panel) => fallback.push({ selectPanel: panel }) }
});
const fallbackHeader = fallback.find((entry) => entry.slot === "conversation.session.header.actions");
assert(fallbackHeader, "no header button without a right sidebar");
fallbackHeader.options.inject().open();
assert(
	fallback.some((entry) => entry.selectPanel === plugin.PANEL_ID),
	"without a right sidebar the header button does not fall back to the centre"
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
