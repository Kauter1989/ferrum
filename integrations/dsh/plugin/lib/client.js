/**
 * FERRUM view panel — a hand-written DeepSeek Harness client bundle.
 *
 * The whole panel is one iframe: the view host (`ferrum-cli view serve`)
 * serves a self-contained page on loopback, and the harness shows it next to
 * the conversation. That page already draws the slice, handles the wheel, the
 * window drag and a pick, and posts the person's actions back, so this bundle
 * only carries the address and the token.
 *
 * Bundle contract (dsh-client-modules):
 *  1. This file is served as a CLASSIC <script>, never an ES module: no
 *     `import`/`export`, no top-level `await`.
 *  2. It calls `window.__ModuleLoader__.load({ id, factory })` exactly once
 *     with `id` equal to the package name in package.json.
 *  3. Nothing runs at load time except that registration; all module bodies
 *     and CSS live inside `factory(require)`.
 *  4. `require` resolves only the harness's platform seed table (react and a
 *     few others); everything else is reached through `ctx`.
 *  5. `factory` returns the exports, which Cordis mounts as the plugin:
 *     `apply(ctx)` plus `inject`.
 */
window.__ModuleLoader__.load({
	id: "ferrum-dsh-panel",
	factory: (require) => {
		var module = { exports: {} };
		var exports = module.exports;
		Object.defineProperty(exports, Symbol.toStringTag, { value: "Module" });

		const React = require("react");
		const h = React.createElement;

		/** Identity of the `main` page and of the sidebar row that selects it. */
		const PANEL_ID = "ferrum-panel";
		/** Where `ferrum-cli view serve` prints its URL by default. */
		const DEFAULT_ORIGIN = "http://127.0.0.1:41234";
		/** The two values the host prints, kept between harness restarts. */
		const KEY_ORIGIN = "ferrum.view.origin";
		const KEY_TOKEN = "ferrum.view.token";

		const styles = {
			page: {
				display: "flex",
				flexDirection: "column",
				height: "100%",
				minHeight: 0,
				padding: "16px",
				gap: "12px",
				boxSizing: "border-box"
			},
			toolbar: { display: "flex", flexWrap: "wrap", gap: "8px", alignItems: "center" },
			input: {
				flex: "1 1 220px",
				minWidth: "160px",
				font: "inherit",
				fontSize: "13px",
				padding: "6px 8px",
				borderRadius: "6px",
				border: "1px solid var(--dsh-border, #d0d5dd)",
				background: "transparent",
				color: "inherit"
			},
			token: { flex: "0 1 260px" },
			button: {
				font: "inherit",
				fontSize: "13px",
				padding: "6px 12px",
				borderRadius: "6px",
				border: "1px solid var(--dsh-border, #d0d5dd)",
				background: "transparent",
				color: "inherit",
				cursor: "pointer"
			},
			stage: {
				flex: "1 1 auto",
				minHeight: 0,
				borderRadius: "10px",
				border: "1px solid var(--dsh-border, #d0d5dd)",
				overflow: "hidden",
				display: "flex",
				alignItems: "stretch",
				justifyContent: "center",
				background: "#0b0d10"
			},
			frame: { width: "100%", height: "100%", border: "0", display: "block" },
			note: { fontSize: "12px", opacity: 0.7 },
			error: { fontSize: "12px", color: "#d92d20" }
		};

		function storage() {
			try {
				return window.localStorage;
			} catch (e) {
				return null;
			}
		}

		function read(key, fallback) {
			const store = storage();
			if (!store) return fallback;
			const value = store.getItem(key);
			return value === null || value === undefined ? fallback : value;
		}

		function write(key, value) {
			const store = storage();
			if (!store) return;
			try {
				store.setItem(key, value);
			} catch (e) {
				/* a private window may refuse */
			}
		}

		/** Loopback only: the host binds 127.0.0.1 and nothing here should reach further. */
		function isLoopback(origin) {
			return /^http:\/\/(127\.0\.0\.1|localhost|\[::1\]):\d{1,5}$/.test(origin);
		}

		function tokenLooksValid(token) {
			return /^[0-9a-f]{32}$/.test(token);
		}

		/** The host's page with the token it printed. */
		function pageUrl(origin, token, nonce) {
			const query = "?token=" + encodeURIComponent(token) + (nonce ? "&n=" + String(nonce) : "");
			return origin + "/" + query;
		}

		/** Sidebar glyph; the sidebar passes `size`. */
		function FerrumIcon({ size }) {
			return h(
				"svg",
				{ width: size, height: size, viewBox: "0 0 16 16", fill: "none", "aria-hidden": "true" },
				h("circle", { cx: 8, cy: 8, r: 6.2, stroke: "currentColor", strokeWidth: 1.3 }),
				h("circle", { cx: 8, cy: 8, r: 2.1, fill: "currentColor" })
			);
		}

		/** The page: two values, a connect button, and the host's viewer below. */
		function FerrumPage() {
			const [origin, setOrigin] = React.useState(read(KEY_ORIGIN, DEFAULT_ORIGIN));
			const [token, setToken] = React.useState(read(KEY_TOKEN, ""));
			const [connected, setConnected] = React.useState(null);
			const [nonce, setNonce] = React.useState(0);
			const [error, setError] = React.useState(null);

			const connect = () => {
				const clean = origin.trim().replace(/\/+$/, "");
				const secret = token.trim();
				if (!isLoopback(clean)) {
					setError("the address must be http://127.0.0.1:<port> — the host listens on loopback only");
					return;
				}
				if (!tokenLooksValid(secret)) {
					setError("the token is the 32 hex digits printed by `ferrum-cli view serve`");
					return;
				}
				write(KEY_ORIGIN, clean);
				write(KEY_TOKEN, secret);
				setError(null);
				setConnected({ origin: clean, token: secret });
				setNonce((n) => n + 1);
			};

			const disconnect = () => {
				write(KEY_TOKEN, "");
				setToken("");
				setConnected(null);
				setError(null);
			};

			return h(
				"div",
				{ style: styles.page },
				h(
					"div",
					{ style: styles.toolbar },
					h("input", {
						style: styles.input,
						value: origin,
						spellCheck: false,
						"aria-label": "View host address",
						placeholder: DEFAULT_ORIGIN,
						onChange: (event) => setOrigin(event.target.value),
						onKeyDown: (event) => {
							if (event.key === "Enter") connect();
						}
					}),
					h("input", {
						style: [styles.input, styles.token],
						value: token,
						spellCheck: false,
						type: "password",
						"aria-label": "View host token",
						placeholder: "token",
						onChange: (event) => setToken(event.target.value),
						onKeyDown: (event) => {
							if (event.key === "Enter") connect();
						}
					}),
					h("button", { type: "button", style: styles.button, onClick: connect }, "Connect"),
					connected
						? h("button", { type: "button", style: styles.button, onClick: () => setNonce((n) => n + 1) }, "Reload")
						: null,
					connected ? h("button", { type: "button", style: styles.button, onClick: disconnect }, "Forget") : null
				),
				h(
					"div",
					{ style: styles.stage },
					connected
						? h("iframe", {
								key: nonce,
								style: styles.frame,
								src: pageUrl(connected.origin, connected.token, nonce),
								title: "FERRUM",
								referrerPolicy: "no-referrer"
							})
						: h(
								"div",
								{ style: styles.note },
								"Run `ferrum-cli view serve --workspace <dir>`, then paste the url and the token it prints."
							)
				),
				error === null
					? h(
							"div",
							{ style: styles.note },
							connected
								? "FERRUM on " + connected.origin + " — wheel: slice, drag: window, click: value"
								: "No view host connected."
						)
					: h("div", { style: styles.error }, error)
			);
		}

		/** Conversation-header button that switches the main column to the page. */
		function FerrumHeaderAction({ open }) {
			return h(
				"button",
				{ type: "button", style: styles.button, title: "Open the FERRUM view", onClick: open },
				"FERRUM"
			);
		}

		/** Cordis services this plugin needs; the fiber waits for each one. */
		const inject = ["slots", "layout"];

		function apply(ctx) {
			ctx.slots.inject("main", () =>
				ctx.slots.register({ name: "main", key: PANEL_ID }, FerrumPage)
			);
			ctx.slots.inject("sidebar.panellist", () =>
				ctx.slots.register(
					{ name: "sidebar.panellist", id: PANEL_ID, order: 10, label: "FERRUM" },
					FerrumIcon
				)
			);
			ctx.slots.inject("conversation.session.header.actions", () =>
				ctx.slots.register(
					{
						name: "conversation.session.header.actions",
						id: "ferrum.open",
						order: 10,
						inject: () => ({ open: () => ctx.layout.selectPanel(PANEL_ID) })
					},
					FerrumHeaderAction
				)
			);
		}

		exports.apply = apply;
		exports.inject = inject;
		exports.PANEL_ID = PANEL_ID;
		exports.isLoopback = isLoopback;
		exports.tokenLooksValid = tokenLooksValid;
		return module.exports;
	}
});
