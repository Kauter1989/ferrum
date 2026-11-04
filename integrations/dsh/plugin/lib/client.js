/**
 * FERRUM view panel — a hand-written DeepSeek Harness client bundle.
 *
 * The panel renders the study itself: it loads FERRUM's WebGPU module
 * (`ferrum_wasm`, built from `crates/ferrum-wasm`) into a `<canvas>` inside the
 * panel and draws MPR (axial, coronal, sagittal) and 3D with the GPU of the
 * machine. Nothing is streamed as images: the voxels are taken once from the
 * view host and every later frame is drawn locally.
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
 *
 * The wasm module is fetched from the view host (`/panel/ferrum_wasm.js` and
 * `/panel/ferrum_wasm_bg.wasm`), because a plugin bundle cannot ship a second
 * script: the host serves the built module next to its own files. Build it with
 * `integrations/dsh/panel/build.cmd`.
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
		const KEY_ORIGIN = "ferrum.view.origin";
		const KEY_TOKEN = "ferrum.view.token";

		const styles = {
			page: { display: "flex", flexDirection: "column", height: "100%", minHeight: 0, padding: "12px", gap: "10px", boxSizing: "border-box" },
			toolbar: { display: "flex", flexWrap: "wrap", gap: "8px", alignItems: "center" },
			input: { flex: "1 1 200px", minWidth: "140px", font: "inherit", fontSize: "13px", padding: "6px 8px", borderRadius: "6px", border: "1px solid var(--dsh-border, #d0d5dd)", background: "transparent", color: "inherit" },
			token: { flex: "0 1 220px" },
			button: { font: "inherit", fontSize: "13px", padding: "6px 12px", borderRadius: "6px", border: "1px solid var(--dsh-border, #d0d5dd)", background: "transparent", color: "inherit", cursor: "pointer" },
			buttonOn: { borderColor: "#4fb3d9", color: "#4fb3d9" },
			stage: { position: "relative", flex: "1 1 auto", minHeight: 0, borderRadius: "10px", border: "1px solid var(--dsh-border, #d0d5dd)", overflow: "hidden", background: "#0b0d10" },
			canvas: { display: "block", width: "100%", height: "100%", cursor: "grab" },
			labels: { position: "absolute", inset: 0, pointerEvents: "none", font: "12px/1.2 ui-monospace, monospace", color: "#c8ccd4" },
			label: { position: "absolute", padding: "2px 4px", background: "rgba(13,14,17,.55)", borderRadius: "4px" },
			note: { fontSize: "12px", opacity: 0.7 },
			error: { fontSize: "12px", color: "#d92d20", whiteSpace: "pre-wrap" }
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

		/**
		 * Loads the wasm module from the view host once per page.
		 *
		 * The module is an ES module with a wasm sibling, and the host serves
		 * both with the right content types, so a dynamic `import()` of its
		 * URL works in the renderer's own realm.
		 */
		let wasmPromise = null;
		function loadWasm(origin) {
			if (!wasmPromise) {
				wasmPromise = import(origin + "/panel/ferrum_wasm.js").then((wasm) =>
					wasm.default(origin + "/panel/ferrum_wasm_bg.wasm").then(() => wasm)
				);
			}
			return wasmPromise;
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

		/** The panel: address and token, a mode bar, and the live canvas. */
		function FerrumPage() {
			const [origin, setOrigin] = React.useState(read(KEY_ORIGIN, DEFAULT_ORIGIN));
			const [token, setToken] = React.useState(read(KEY_TOKEN, ""));
			const [mode, setMode] = React.useState("mpr");
			const [renderMode, setRenderMode] = React.useState("transfer_function");
			const [status, setStatus] = React.useState(
				tokenLooksValid(read(KEY_TOKEN, ""))
					? "a saved token is filled in — a host prints a new one every time it starts"
					: "no view host connected"
			);
			const [error, setError] = React.useState(null);
			const canvasRef = React.useRef(null);
			const viewRef = React.useRef(null);
			const wasmRef = React.useRef(null);
			const sliceRef = React.useRef(0);
			const modeRef = React.useRef("mpr");

			modeRef.current = mode;

			/** One frame of the loop: sizes the drawing buffer and draws. */
			const draw = React.useCallback(() => {
				const view = viewRef.current;
				const canvas = canvasRef.current;
				if (!view || !canvas) return;
				const dpr = window.devicePixelRatio || 1;
				const rect = canvas.getBoundingClientRect();
				const width = Math.max(1, Math.round(rect.width * dpr));
				const height = Math.max(1, Math.round(rect.height * dpr));
				if (canvas.width !== width || canvas.height !== height) {
					canvas.width = width;
					canvas.height = height;
					view.resize(width, height);
				}
				try {
					view.render();
				} catch (e) {
					setError(e && e.message ? e.message : String(e));
					return;
				}
				requestAnimationFrame(draw);
			}, []);

			const connect = React.useCallback(async () => {
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
				setStatus("loading the renderer…");
				try {
					const wasm = await loadWasm(clean);
					wasmRef.current = wasm;
					const canvas = canvasRef.current;
					if (!canvas) throw new Error("the canvas is not ready yet");
					const view = await wasm.FerrumView.create(canvas);
					viewRef.current = view;
					setStatus("taking the study…");
					// A stale token is the one failure a person can fix, so it is
					// named: the host prints a new one on every start, and the panel
					// remembers the old one in localStorage.
					const take = async (path, want) => {
						const response = await fetch(clean + path + "?token=" + encodeURIComponent(secret));
						if (response.status === 401) {
							throw new Error(
								"the view host refused the token (401). Each `ferrum-cli view serve` prints a new one: " +
									"paste the token from the terminal that runs it and press Connect again."
							);
						}
						if (!response.ok) throw new Error(path + ": HTTP " + response.status);
						return want === "json" ? response.json() : response.arrayBuffer();
					};
					const header = await take("/api/volume.json", "json");
					const bytes = await take("/api/volume", "bytes");
					view.set_volume(new Uint32Array(header.dims), new Float32Array(header.spacing_mm), new Float32Array(bytes));
					view.set_window(header.value_range[0], header.value_range[1]);
					// A CT is shown with the desktop's own preset: `tissue` has its
					// own palette, so the transfer-function mode is the one that
					// follows `set_ct_preset`.
					try {
						view.set_ct_preset("soft_tissue_bone");
						view.set_render_mode(renderMode);
					} catch (presetError) {
						/* a non-CT volume keeps the default ramp */
					}
					sliceRef.current = header.dims[2] / 2;
					view.set_mode(modeRef.current === "volume" ? wasm.FerrumMode.Volume3d : modeRef.current === "mpr" ? wasm.FerrumMode.Mpr : wasm.FerrumMode.Slice2d);
					view.set_render_mode(renderMode);
					setStatus(view.adapter_info() + " · " + header.dims.join("×") + " · " + header.value_unit);
					draw();
				} catch (e) {
					setError(e && e.message ? e.message : String(e));
					setStatus("failed");
				}
			}, [origin, token, renderMode, draw]);

			// Pointer and wheel drive the 3D camera or the axial slice.
			React.useEffect(() => {
				const canvas = canvasRef.current;
				if (!canvas) return undefined;
				let dragging = null;
				const onDown = (e) => {
					canvas.setPointerCapture(e.pointerId);
					dragging = { x: e.clientX, y: e.clientY };
					canvas.style.cursor = "grabbing";
				};
				const onMove = (e) => {
					const view = viewRef.current;
					if (!dragging || !view) return;
					const dx = e.clientX - dragging.x;
					const dy = e.clientY - dragging.y;
					dragging = { x: e.clientX, y: e.clientY };
					if (modeRef.current !== "slice") view.orbit(dx * 0.01, dy * 0.01);
				};
				const onUp = () => {
					dragging = null;
					canvas.style.cursor = "grab";
				};
				const onWheel = (e) => {
					const view = viewRef.current;
					if (!view) return;
					e.preventDefault();
					if (modeRef.current !== "slice") {
						view.zoom(e.deltaY > 0 ? -0.08 : 0.08);
						return;
					}
					sliceRef.current = Math.max(0, sliceRef.current + (e.deltaY > 0 ? 1 : -1));
					try {
						view.set_slice("axial", sliceRef.current);
					} catch (err) {
						/* the volume may not be loaded yet */
					}
				};
				canvas.addEventListener("pointerdown", onDown);
				canvas.addEventListener("pointermove", onMove);
				canvas.addEventListener("pointerup", onUp);
				canvas.addEventListener("wheel", onWheel, { passive: false });
				return () => {
					canvas.removeEventListener("pointerdown", onDown);
					canvas.removeEventListener("pointermove", onMove);
					canvas.removeEventListener("pointerup", onUp);
					canvas.removeEventListener("wheel", onWheel);
				};
			}, []);

			// The mode bar drives the renderer's layout.
			React.useEffect(() => {
				const view = viewRef.current;
				const wasm = wasmRef.current;
				if (!view || !wasm) return;
				try {
					view.set_mode(mode === "volume" ? wasm.FerrumMode.Volume3d : mode === "mpr" ? wasm.FerrumMode.Mpr : wasm.FerrumMode.Slice2d);
				} catch (e) {
					setError(e && e.message ? e.message : String(e));
				}
			}, [mode]);

			React.useEffect(() => {
				const view = viewRef.current;
				if (!view) return;
				try {
					view.set_render_mode(renderMode);
				} catch (e) {
					setError(e && e.message ? e.message : String(e));
				}
			}, [renderMode]);

			const button = (label, on, onClick) =>
				h("button", { type: "button", style: on ? Object.assign({}, styles.button, styles.buttonOn) : styles.button, onClick }, label);

			return h(
				"div",
				{ style: styles.page },
				h(
					"div",
					{ style: styles.toolbar },
					h("input", { style: styles.input, value: origin, spellCheck: false, "aria-label": "View host address", onChange: (e) => setOrigin(e.target.value) }),
					h("input", { style: Object.assign({}, styles.input, styles.token), value: token, spellCheck: false, type: "password", "aria-label": "View host token", placeholder: "token", onChange: (e) => setToken(e.target.value) }),
					h("button", { type: "button", style: styles.button, onClick: connect }, "Connect"),
					button("MPR", mode === "mpr", () => setMode("mpr")),
					button("3D", mode === "volume", () => setMode("volume")),
					button("2D", mode === "slice", () => setMode("slice")),
					h(
						"select",
						{ style: styles.button, value: renderMode, onChange: (e) => setRenderMode(e.target.value), "aria-label": "3D mode" },
						["tissue", "isosurface", "mip", "transfer_function"].map((m) => h("option", { key: m, value: m }, m))
					)
				),
				h(
					"div",
					{ style: styles.stage },
					h("canvas", { ref: canvasRef, style: styles.canvas }),
					mode === "mpr"
						? h(
								"div",
								{ style: styles.labels },
								h("span", { style: Object.assign({}, styles.label, { left: 8, top: 8 }) }, "axial"),
								h("span", { style: Object.assign({}, styles.label, { left: "50.5%", top: 8 }) }, "coronal"),
								h("span", { style: Object.assign({}, styles.label, { left: 8, top: "50.5%" }) }, "sagittal"),
								h("span", { style: Object.assign({}, styles.label, { left: "50.5%", top: "50.5%" }) }, "3D " + renderMode)
							)
						: null
				),
				error === null ? h("div", { style: styles.note }, status) : h("div", { style: styles.error }, error)
			);
		}

		/** Conversation-header button that switches the main column to the page. */
		function FerrumHeaderAction({ open }) {
			return h("button", { type: "button", style: styles.button, title: "Open the FERRUM view", onClick: open }, "FERRUM");
		}

		/** The caption of the FERRUM tab in the right sidebar's strip. */
		function FerrumTabTitle() {
			return h("span", null, "FERRUM");
		}

		/** Cordis services this plugin needs; the fiber waits for each one. */
		const inject = ["slots", "layout", "sidebarRight", "sidebarRightTabs"];

		function apply(ctx) {
			ctx.slots.inject("main", () => ctx.slots.register({ name: "main", key: PANEL_ID }, FerrumPage));
			ctx.slots.inject("sidebar.panellist", () =>
				ctx.slots.register({ name: "sidebar.panellist", id: PANEL_ID, order: 10, label: "FERRUM" }, FerrumIcon)
			);

			// A tab of the right sidebar: the conversation keeps the centre
			// column and the study sits beside it. DSH owns the tab strip, the
			// pane actions and the session scoping; this plugin only says what a
			// FERRUM tab is and what it draws.
			//
			// When a build has no right sidebar, the panel still works in the
			// main column: the services are checked, not assumed.
			if (ctx.sidebarRightTabs !== undefined && ctx.sidebarRight !== undefined) {
				ctx.sidebarRightTabs.register({
					id: PANEL_ID,
					kind: PANEL_ID,
					multiple: false,
					title: () => "FERRUM",
					// The strip's "+" menu lists these entries: a type with no guide
					// entry is registered but offered nowhere.
					guide: [
						{
							id: "open",
							title: () => "FERRUM",
							description: () => "View the study: MPR and 3D",
							icon: FerrumIcon
						}
					]
				});
				ctx.slots.inject("sidebar.right.pane.tab", () =>
					ctx.slots.register({ name: "sidebar.right.pane.tab", key: PANEL_ID }, FerrumPage)
				);
				ctx.slots.inject("sidebar.right.pane.tab.title", () =>
					ctx.slots.register({ name: "sidebar.right.pane.tab.title", key: PANEL_ID }, FerrumTabTitle)
				);
			}

			ctx.slots.inject("conversation.session.header.actions", () =>
				ctx.slots.register(
					{
						name: "conversation.session.header.actions",
						id: "ferrum.open",
						order: 10,
						// The header button puts the panel beside the conversation: expand
						// the right sidebar if it is collapsed, then open the tab in the
						// session the focused surface belongs to (the same path the
						// built-in Browser takes). Without a right sidebar, or without
						// a session to bind to, the main column shows the panel.
						inject: () => ({
							open: () => {
								try {
									const target = ctx.sidebarRight.commandTarget(document.activeElement);
									if (target !== undefined) {
										ctx.sidebarRight.openTabFromTarget(PANEL_ID, target);
										// A collapsed pane holds the tab but shows nothing,
										// so reveal it. A build whose pane cannot report its
										// state simply keeps whatever the person chose.
										try {
											if (typeof ctx.sidebarRight.isExpanded === "function" && !ctx.sidebarRight.isExpanded()) {
												ctx.sidebarRight.toggleExpanded();
											}
										} catch (e) {
											/* the tab is open; the pane keeps its own state */
										}
										return;
									}
								} catch (e) {
									/* fall through to the main column */
								}
								ctx.layout.selectPanel(PANEL_ID);
							}
						})
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