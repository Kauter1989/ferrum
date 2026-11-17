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
			// A native drop-down paints its popup with the platform's palette and
			// does not inherit the page's colours: with `color: inherit` the options
			// computed to near-white on a transparent background, and the popup drew
			// them white on white. Both the list and its options state their own
			// colours, and `color-scheme: light` makes the popup follow them.
			select: { font: "inherit", fontSize: "13px", padding: "6px 8px", borderRadius: "6px", border: "1px solid var(--dsh-border, #d0d5dd)", background: "#ffffff", color: "#1d1f26", cursor: "pointer", colorScheme: "light" },
			option: { background: "#ffffff", color: "#1d1f26" },
			stage: { position: "relative", flex: "1 1 auto", minHeight: 0, borderRadius: "10px", border: "1px solid var(--dsh-border, #d0d5dd)", overflow: "hidden", background: "#0b0d10" },
			canvas: { display: "block", width: "100%", height: "100%", cursor: "grab" },
			measureCanvas: { position: "absolute", inset: 0, width: "100%", height: "100%", pointerEvents: "none" },
			labels: { position: "absolute", inset: 0, pointerEvents: "none", font: "12px/1.2 ui-monospace, monospace", color: "#c8ccd4" },
			label: { position: "absolute", padding: "2px 4px", background: "rgba(13,14,17,.55)", borderRadius: "4px" },
			note: { fontSize: "12px", opacity: 0.7 },
			error: { fontSize: "12px", color: "#d92d20", whiteSpace: "pre-wrap" },
			tfRow: { display: "flex", gap: "10px", alignItems: "center", flexWrap: "wrap" },
			tfCanvas: { width: "280px", height: "130px", borderRadius: "8px", border: "1px solid var(--dsh-border, #d0d5dd)", cursor: "crosshair", touchAction: "none" },
			tfSide: { display: "flex", flexDirection: "column", gap: "6px", alignItems: "flex-start" }
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

		/** The plot area of the transfer-function editor canvas (pixels). */
		function tfPlot(w, h) {
			return { x: 6, y: 6, w: w - 12, h: h - 30 };
		}

		/** A control point's colour as a CSS rgba string. */
		function tfCssColor(p, alpha) {
			const u = (v) => Math.round(Math.max(0, Math.min(1, v)) * 255);
			return "rgba(" + u(p.r) + "," + u(p.g) + "," + u(p.b) + "," + alpha + ")";
		}

		/** A control point's colour as a #rrggbb string for the colour input. */
		function tfHexColor(p) {
			const u = (v) => Math.round(Math.max(0, Math.min(1, v)) * 255).toString(16).padStart(2, "0");
			return "#" + u(p.r) + u(p.g) + u(p.b);
		}

		/**
		 * Paints the editor: the histogram background, the colour strip, the
		 * opacity curve and the handles. This is UI chrome on a 2D canvas —
		 * the volume itself is still drawn by the module's existing shaders.
		 */
		function drawTfEditor(ctx, w, h, points, hist, selected) {
			const plot = tfPlot(w, h);
			ctx.fillStyle = "#0d0e11";
			ctx.fillRect(0, 0, w, h);
			if (hist.length > 0) {
				ctx.fillStyle = "#2c3e5c";
				const bw = plot.w / hist.length;
				for (let i = 0; i < hist.length; i++) {
					const bh = Math.max(0, Math.min(1, hist[i])) * plot.h;
					ctx.fillRect(plot.x + i * bw, plot.y + plot.h - bh, Math.max(bw, 1), bh);
				}
			}
			// The colour strip: the browser interpolates between the point
			// colours, so no sampling of the function is copied here.
			const stripY = plot.y + plot.h + 4;
			const grad = ctx.createLinearGradient(plot.x, 0, plot.x + plot.w, 0);
			for (const p of points) grad.addColorStop(Math.max(0, Math.min(1, p.x)), tfCssColor(p, 1));
			ctx.fillStyle = grad;
			ctx.fillRect(plot.x, stripY, plot.w, h - stripY - 4);
			ctx.strokeStyle = "#4fb3d9";
			ctx.lineWidth = 1.5;
			ctx.beginPath();
			points.forEach((p, i) => {
				const x = plot.x + p.x * plot.w;
				const y = plot.y + (1 - p.a) * plot.h;
				if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
			});
			ctx.stroke();
			points.forEach((p, i) => {
				const x = plot.x + p.x * plot.w;
				const y = plot.y + (1 - p.a) * plot.h;
				ctx.beginPath();
				ctx.arc(x, y, 5, 0, Math.PI * 2);
				ctx.fillStyle = tfCssColor(p, 1);
				ctx.fill();
				ctx.lineWidth = i === selected ? 2.5 : 1;
				ctx.strokeStyle = i === selected ? "#4fb3d9" : "#ffffff";
				ctx.stroke();
			});
		}

		/**
		 * The transfer-function editor (29.2-b): draggable control points over
		 * the volume's histogram, with a colour picker for the selected point.
		 * Every edit goes through the module's `tf_*` bindings, which re-bake
		 * the lookup table the existing volume shader draws with, so the 3D
		 * view follows on its next frame. Double-click adds a point,
		 * right-click removes an interior one; the endpoints stay at 0 and 1.
		 */
		function TfEditor({ view }) {
			const canvasRef = React.useRef(null);
			const dragRef = React.useRef(null);
			const [points, setPoints] = React.useState([]);
			const [hist, setHist] = React.useState([]);
			const [selected, setSelected] = React.useState(null);

			const reload = React.useCallback(() => {
				try {
					const flat = view.tf_points();
					const pts = [];
					for (let i = 0; i + 4 < flat.length; i += 5) {
						pts.push({ x: flat[i], a: flat[i + 1], r: flat[i + 2], g: flat[i + 3], b: flat[i + 4] });
					}
					setPoints(pts);
					setHist(Array.from(view.tf_histogram()));
				} catch (e) {
					/* the view is not ready yet */
				}
			}, [view]);
			React.useEffect(() => { reload(); }, [reload]);

			React.useEffect(() => {
				const canvas = canvasRef.current;
				if (!canvas) return;
				drawTfEditor(canvas.getContext("2d"), canvas.width, canvas.height, points, hist, selected);
			}, [points, hist, selected]);

			/** The pointer in the editor's unit square (x intensity, a opacity). */
			const unitAt = (e) => {
				const canvas = canvasRef.current;
				const rect = canvas.getBoundingClientRect();
				const plot = tfPlot(canvas.width, canvas.height);
				const px = (e.clientX - rect.left) * (canvas.width / rect.width);
				const py = (e.clientY - rect.top) * (canvas.height / rect.height);
				return {
					x: Math.max(0, Math.min(1, (px - plot.x) / plot.w)),
					a: Math.max(0, Math.min(1, (plot.y + plot.h - py) / plot.h))
				};
			};

			const edit = (fn) => {
				try {
					fn();
					reload();
				} catch (e) {
					/* the module refuses an invalid edit and keeps the curve */
				}
			};

			const onDown = (e) => {
				if (e.button !== 0) return;
				const u = unitAt(e);
				const hit = view.tf_hit_test(u.x, u.a, 0.06);
				if (hit == null) return;
				dragRef.current = hit;
				setSelected(hit);
				canvasRef.current.setPointerCapture(e.pointerId);
			};
			const onMove = (e) => {
				if (dragRef.current == null) return;
				const u = unitAt(e);
				const index = dragRef.current;
				edit(() => view.tf_move_point(index, u.x, u.a));
			};
			const onUp = () => { dragRef.current = null; };
			const onDblClick = (e) => {
				const u = unitAt(e);
				if (view.tf_hit_test(u.x, u.a, 0.06) != null) return;
				edit(() => {
					const index = view.tf_insert_point(u.x);
					view.tf_move_point(index, u.x, u.a);
					setSelected(index);
				});
			};
			const onContextMenu = (e) => {
				e.preventDefault();
				const u = unitAt(e);
				const hit = view.tf_hit_test(u.x, u.a, 0.06);
				if (hit == null || hit === 0 || hit === points.length - 1) return;
				edit(() => view.tf_remove_point(hit));
				setSelected(null);
			};

			const selectedPoint = selected != null && selected < points.length ? points[selected] : null;
			const onColor = (e) => {
				if (selected == null) return;
				const hex = e.target.value;
				const channel = (i) => parseInt(hex.slice(1 + i * 2, 3 + i * 2), 16) / 255;
				edit(() => view.tf_set_color(selected, channel(0), channel(1), channel(2)));
			};

			return h(
				"div", { style: styles.tfRow },
				h("canvas", {
					ref: canvasRef, width: 280, height: 130, style: styles.tfCanvas,
					"aria-label": "Transfer function editor",
					onPointerDown: onDown, onPointerMove: onMove, onPointerUp: onUp,
					onDoubleClick: onDblClick, onContextMenu: onContextMenu
				}),
				h(
					"div", { style: styles.tfSide },
					h("input", {
						type: "color", value: selectedPoint ? tfHexColor(selectedPoint) : "#ffffff",
						onChange: onColor, disabled: selectedPoint === null,
						"aria-label": "Point colour", title: "Colour of the selected point"
					}),
					h("span", { style: styles.note }, "drag points · double-click adds · right-click removes")
				)
			);
		}

		/** A measured value with its unit, as the panel displays it (29.2-c). */
		function formatMeasure(value, unit) {
			if (unit === "mm") return value.toFixed(1) + " mm";
			if (unit === "deg") return value.toFixed(1) + "°";
			if (unit === "mm2") return value.toFixed(2) + " mm²";
			return String(value);
		}

		/** Distance between two slice pixels, in millimetres. */
		function mmDistance(a, b, spacing) {
			const dx = (a[0] - b[0]) * spacing[0];
			const dy = (a[1] - b[1]) * spacing[1];
			return Math.sqrt(dx * dx + dy * dy);
		}

		/** Where an annotation's value label sits, in screen pixels. */
		function labelAnchor(kind, pts) {
			if (kind === "Angle" && pts.length >= 2) return pts[1];
			if (kind === "Area" && pts.length > 0) {
				const sum = pts.reduce((acc, p) => [acc[0] + p[0], acc[1] + p[1]], [0, 0]);
				return [sum[0] / pts.length, sum[1] / pts.length];
			}
			if (pts.length >= 2) return [(pts[0][0] + pts[1][0]) / 2, (pts[0][1] + pts[1][1]) / 2];
			return pts[0] || [0, 0];
		}

		/**
		 * Draws one committed annotation over the slice: its outline and its
		 * measured value. The shape comes from the host's in-plane millimetre
		 * points; the value is the host's own — the panel measures nothing.
		 */
		function drawAnnotation(ctx, a, toScreen) {
			const pts = (a.points_mm || []).map(toScreen);
			if (pts.length === 0) return;
			ctx.strokeStyle = "#ffd166";
			ctx.fillStyle = "#ffd166";
			ctx.beginPath();
			if (a.kind === "Rectangle" && pts.length === 2) {
				ctx.rect(
					Math.min(pts[0][0], pts[1][0]), Math.min(pts[0][1], pts[1][1]),
					Math.abs(pts[1][0] - pts[0][0]), Math.abs(pts[1][1] - pts[0][1])
				);
			} else {
				pts.forEach((p, i) => { if (i === 0) ctx.moveTo(p[0], p[1]); else ctx.lineTo(p[0], p[1]); });
				if (a.kind === "Area") ctx.closePath();
			}
			ctx.stroke();
			const text = a.value != null ? formatMeasure(a.value, a.unit) : (a.label || a.name || "");
			const anchor = labelAnchor(a.kind, pts);
			ctx.fillText(text, anchor[0] + 6, anchor[1] - 6);
		}

		/** Draws the in-progress measurement (the drag preview) in blue. */
		function drawDraft(ctx, draft, toScreen) {
			const pts = draft.points.map(toScreen);
			const hover = draft.hover ? toScreen(draft.hover) : null;
			ctx.strokeStyle = "#4fb3d9";
			ctx.fillStyle = "#4fb3d9";
			ctx.beginPath();
			if (draft.tool === "rect" && pts.length === 1 && hover) {
				ctx.rect(
					Math.min(pts[0][0], hover[0]), Math.min(pts[0][1], hover[1]),
					Math.abs(hover[0] - pts[0][0]), Math.abs(hover[1] - pts[0][1])
				);
			} else {
				const all = hover ? pts.concat([hover]) : pts;
				all.forEach((p, i) => { if (i === 0) ctx.moveTo(p[0], p[1]); else ctx.lineTo(p[0], p[1]); });
				if (draft.tool === "area" && all.length >= 3) ctx.closePath();
			}
			ctx.stroke();
			for (const p of pts) {
				ctx.beginPath();
				ctx.arc(p[0], p[1], 3, 0, Math.PI * 2);
				ctx.fill();
			}
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
			const sliceRef = React.useRef(0);
			const modeRef = React.useRef("mpr");

			modeRef.current = mode;

			// The segment overlay display state, mirrored from the host and sent
			// back when the person changes it.
			const [overlayShown, setOverlayShown] = React.useState(false);
			const [overlayStyle, setOverlayStyle] = React.useState("fill_outline");
			const [overlayFill, setOverlayFill] = React.useState(0.5);
			const overlayRef = React.useRef({ shown: false, fill: 0.5, style: "fill_outline" });

			// Measurements (29.2-c): the tool machine stays in the host. The
			// panel collects slice pixels, sends a `draw` input and displays the
			// values the host computed (/api/annotations) over its own slice.
			const [tool, setTool] = React.useState("navigate");
			const [annotations, setAnnotations] = React.useState([]);
			const toolRef = React.useRef("navigate");
			const annotationsRef = React.useRef([]);
			const draftRef = React.useRef(null);
			const dimsRef = React.useRef(null);
			const measureCanvasRef = React.useRef(null);
			const pollTimer = React.useRef(null);
			toolRef.current = tool;

			/**
			 * The measurement overlay: committed annotations of the shown slice
			 * and the in-progress draft, on a 2D canvas above the WebGPU one.
			 * Runs in the frame loop, so it follows resize and slice scrolls.
			 */
			const drawMeasureOverlay = React.useCallback(() => {
				const canvas = measureCanvasRef.current;
				const info = dimsRef.current;
				if (!canvas || !info) return;
				const rect = canvas.getBoundingClientRect();
				const dpr = window.devicePixelRatio || 1;
				const w = Math.max(1, Math.round(rect.width * dpr));
				const h = Math.max(1, Math.round(rect.height * dpr));
				if (canvas.width !== w || canvas.height !== h) {
					canvas.width = w;
					canvas.height = h;
				}
				const ctx = canvas.getContext("2d");
				ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
				ctx.clearRect(0, 0, rect.width, rect.height);
				if (modeRef.current !== "slice") return;
				// The 2D view stretches the axial slice over the canvas, so both
				// the host's millimetre points and the draft's pixels map linearly.
				const mmW = info.dims[0] * info.spacing[0];
				const mmH = info.dims[1] * info.spacing[1];
				const toScreen = (mm) => [(mm[0] / mmW) * rect.width, (mm[1] / mmH) * rect.height];
				const pxToScreen = (px) => [(px[0] / info.dims[0]) * rect.width, (px[1] / info.dims[1]) * rect.height];
				ctx.lineWidth = 1.5;
				ctx.font = "12px ui-monospace, monospace";
				for (const a of annotationsRef.current) {
					if (a.plane === "axial" && a.slice_index === sliceRef.current) drawAnnotation(ctx, a, toScreen);
				}
				if (draftRef.current) drawDraft(ctx, draftRef.current, pxToScreen);
			}, []);

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
				drawMeasureOverlay();
				requestAnimationFrame(draw);
			}, [drawMeasureOverlay]);

			// The host's token is short-lived and rotates (ADR 0019). The panel
		// refreshes it proactively so a reconnect — and the state sync to come —
		// always has a live token; the boot token printed by `view serve` only
		// starts the session.
		const tokenRef = React.useRef(token);
		tokenRef.current = token;
		const originRef = React.useRef(origin);
		originRef.current = origin;
		const refreshTimer = React.useRef(null);
		const sessionGen = React.useRef(0);
		const refreshToken = React.useCallback(async () => {
			const gen = sessionGen.current;
			const clean = originRef.current.trim().replace(/\/+$/, "");
			const secret = tokenRef.current.trim();
			if (!isLoopback(clean) || !tokenLooksValid(secret)) return;
			try {
				const response = await fetch(clean + "/api/token/refresh?token=" + encodeURIComponent(secret), { method: "POST" });
				if (!response.ok) return; // expired or host gone: a reconnect needs a fresh boot token
				const reply = await response.json();
				if (reply && reply.token && sessionGen.current === gen) {
					write(KEY_TOKEN, reply.token);
					setToken(reply.token);
					const ttlMs = (reply.expires_in || 900) * 1000;
					refreshTimer.current = setTimeout(refreshToken, Math.max(ttlMs * 0.8, 5000));
				}
			} catch (e) {
				/* host unreachable: stop refreshing until the next Connect */
			}
		}, []);
		React.useEffect(() => () => clearTimeout(refreshTimer.current), []);

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
				draftRef.current = null;
				annotationsRef.current = [];
				setAnnotations([]);
				setStatus("loading the renderer…");
				try {
					const wasm = await loadWasm(clean);
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
					// The default wire form is u16 storage (half the bytes); reconstruct
					// the physical values from value_range. f32 stays supported.
					let voxels;
					if (header.storage === "f32") {
						voxels = new Float32Array(bytes);
					} else {
						const stored = new Uint16Array(bytes);
						const min = header.value_range[0], span = header.value_range[1] - min;
						voxels = new Float32Array(stored.length);
						for (let i = 0; i < stored.length; i++) voxels[i] = min + stored[i] * span / 65535;
					}
					view.set_volume(new Uint32Array(header.dims), new Float32Array(header.spacing_mm), voxels);
					view.set_window(header.value_range[0], header.value_range[1]);
					// The measurement overlay maps the host's millimetre points and
					// the draft's pixels through the grid it was loaded with.
					dimsRef.current = { dims: header.dims, spacing: header.spacing_mm };
					// A CT is shown with the desktop's own preset: `tissue` has its
					// own palette, so the transfer-function mode is the one that
					// follows `set_ct_preset`.
					try {
						view.set_ct_preset("soft_tissue_bone");
						view.set_render_mode(renderMode);
					} catch (presetError) {
						/* a non-CT volume keeps the default ramp */
					}
					sliceRef.current = Math.floor(header.dims[2] / 2);
					// The mode goes through by name: the module validates it and refuses
					// an unknown value, where a local ternary would fall back to a
					// single 2D slice without saying so (F1).
					view.set_mode(modeRef.current);
					view.set_render_mode(renderMode);
					// Segments and their overlay come from the host: the label grid
					// (/api/labels) and the segment metadata + display state
					// (/api/state). The existing shaders draw the overlay.
					try {
						const st = await take("/api/state", "json");
						const labBytes = await take("/api/labels", "bytes");
						view.set_labels(new Uint32Array(header.dims), new Uint8Array(labBytes));
						const items = (st.segments && st.segments.items) || [];
						for (const seg of items) {
							try {
								view.set_segment(
									seg.label,
									seg.name || "Segment " + seg.label,
									new Uint8Array(seg.color || [230, 85, 75]),
									seg.visible !== false,
									seg.opacity != null ? seg.opacity : 0.5
								);
							} catch (segErr) {
								/* an empty segment (its label is absent from the grid) is
								   skipped, so one bad segment cannot disable the overlay */
							}
						}
						const ov = st.overlays || {};
						const shown = ov.segments_shown !== false && items.length > 0;
						const fillOpacity = ov.fill_opacity != null ? ov.fill_opacity : 0.5;
						const style = ov.style || "fill_outline";
						view.set_overlay(shown, fillOpacity, style);
						overlayRef.current = { shown, fill: fillOpacity, style };
						setOverlayShown(shown);
						setOverlayStyle(style);
						setOverlayFill(fillOpacity);
					} catch (labErr) {
						/* no segmentation or host state: the overlay stays off */
					}
					setStatus(view.adapter_info() + " · " + header.dims.join("×") + " · " + header.value_unit);
					draw();
					// The measurements the host already holds, then a light poll so
					// an agent's drawn annotations appear too (the endpoint refreshes
					// the workspace on every read).
					fetchAnnotations();
					clearInterval(pollTimer.current);
					const gen = sessionGen.current + 1;
					pollTimer.current = setInterval(() => {
						if (sessionGen.current === gen) fetchAnnotations();
					}, 5000);
					// start keeping the token alive (the first refresh learns the TTL)
					clearTimeout(refreshTimer.current);
					sessionGen.current += 1;
					refreshTimer.current = setTimeout(refreshToken, 5000);
				} catch (e) {
					setError(e && e.message ? e.message : String(e));
					setStatus("failed");
				}
			}, [origin, token, renderMode, draw]);

			// Send a view input to the host (the person's change). The host is the
			// source of truth; the local renderer is kept in step optimistically.
			// sendInput answers with the new state (or null); pushInput is the
			// fire-and-forget form for display changes.
			const sendInput = React.useCallback((body) => {
				const clean = originRef.current.trim().replace(/\/+$/, "");
				const secret = tokenRef.current.trim();
				if (!isLoopback(clean) || !tokenLooksValid(secret)) return Promise.resolve(null);
				return fetch(clean + "/api/input?token=" + encodeURIComponent(secret), {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify(body)
				})
					.then((r) => (r.ok ? r.json() : null))
					.catch(() => null);
			}, []);
			const pushInput = React.useCallback((body) => { sendInput(body); }, [sendInput]);

			/** Read the host's measured values (the panel computes none itself). */
			const fetchAnnotations = React.useCallback(async () => {
				const clean = originRef.current.trim().replace(/\/+$/, "");
				const secret = tokenRef.current.trim();
				if (!isLoopback(clean) || !tokenLooksValid(secret)) return;
				try {
					const response = await fetch(clean + "/api/annotations?token=" + encodeURIComponent(secret));
					if (!response.ok) return;
					const doc = await response.json();
					annotationsRef.current = doc.annotations || [];
					setAnnotations(annotationsRef.current);
				} catch (e) {
					/* host unreachable: the last values stay on screen */
				}
			}, []);
			React.useEffect(() => () => clearInterval(pollTimer.current), []);

			/**
			 * Commit a measurement: the host measures on its active plane's
			 * slice, so the panel's axial plane and slice go first (in order),
			 * then the draw; the values come back from /api/annotations.
			 */
			const commitDraw = React.useCallback(async (toolName, points) => {
				await sendInput({ plane: "axial" });
				await sendInput({ slice: sliceRef.current });
				await sendInput({ draw: { tool: toolName, points } });
				draftRef.current = null;
				await fetchAnnotations();
			}, [sendInput, fetchAnnotations]);

			/** The pointer as a pixel of the axial slice the 2D view shows. */
			const slicePixelAt = (e) => {
				const canvas = canvasRef.current;
				const info = dimsRef.current;
				if (!canvas || !info) return null;
				const rect = canvas.getBoundingClientRect();
				const x = Math.floor(((e.clientX - rect.left) / rect.width) * info.dims[0]);
				const y = Math.floor(((e.clientY - rect.top) / rect.height) * info.dims[1]);
				return [
					Math.max(0, Math.min(info.dims[0] - 1, x)),
					Math.max(0, Math.min(info.dims[1] - 1, y))
				];
			};

			// Pointer and wheel drive the 3D camera, the axial slice, or a
			// measurement draft when a tool is active in the 2D view.
			React.useEffect(() => {
				const canvas = canvasRef.current;
				if (!canvas) return undefined;
				let dragging = null;
				const spacing = () => (dimsRef.current ? dimsRef.current.spacing : [1, 1, 1]);
				const onDown = (e) => {
					const toolName = toolRef.current;
					if (modeRef.current === "slice" && toolName !== "navigate") {
						const px = slicePixelAt(e);
						if (!px) return;
						canvas.setPointerCapture(e.pointerId);
						canvas.style.cursor = "crosshair";
						if (toolName === "distance" || toolName === "rect") {
							draftRef.current = { tool: toolName, points: [px], hover: px };
							return;
						}
						const draft =
							draftRef.current && draftRef.current.tool === toolName
								? draftRef.current
								: { tool: toolName, points: [], hover: px };
						const pts = draft.points;
						// Area closes when a click lands near the first point — the
						// host tool's own 6 mm rule; a repeated click adds nothing.
						if (toolName === "area" && pts.length >= 3 && mmDistance(pts[0], px, spacing()) <= 6.0) {
							commitDraw(toolName, pts);
							return;
						}
						const last = pts[pts.length - 1];
						if (!last || last[0] !== px[0] || last[1] !== px[1]) pts.push(px);
						draft.hover = px;
						draftRef.current = draft;
						if (toolName === "angle" && pts.length === 3) commitDraw(toolName, pts);
						return;
					}
					canvas.setPointerCapture(e.pointerId);
					dragging = { x: e.clientX, y: e.clientY };
					canvas.style.cursor = "grabbing";
				};
				const onMove = (e) => {
					const draft = draftRef.current;
					if (draft) {
						const px = slicePixelAt(e);
						if (px) draft.hover = px;
						return;
					}
					const view = viewRef.current;
					if (!dragging || !view) return;
					const dx = e.clientX - dragging.x;
					const dy = e.clientY - dragging.y;
					dragging = { x: e.clientX, y: e.clientY };
					if (modeRef.current !== "slice") view.orbit(dx * 0.01, dy * 0.01);
				};
				const onUp = (e) => {
					const draft = draftRef.current;
					if (draft && (draft.tool === "distance" || draft.tool === "rect")) {
						const px = slicePixelAt(e) || draft.hover;
						const start = draft.points[0];
						// The host commits a distance/rect from 0.5 mm on; a smaller
						// drag (or a plain click) draws nothing.
						if (px && mmDistance(start, px, spacing()) >= 0.5) {
							commitDraw(draft.tool, [start, px]);
						} else {
							draftRef.current = null;
						}
						canvas.style.cursor = "crosshair";
						return;
					}
					dragging = null;
					canvas.style.cursor = toolRef.current !== "navigate" && modeRef.current === "slice" ? "crosshair" : "grab";
				};
				const onDblClick = () => {
					const draft = draftRef.current;
					if (draft && draft.tool === "area" && draft.points.length >= 3) commitDraw(draft.tool, draft.points);
				};
				const onContextMenu = (e) => {
					if (draftRef.current) {
						e.preventDefault();
						draftRef.current = null;
					}
				};
				const onWheel = (e) => {
					const view = viewRef.current;
					if (!view) return;
					e.preventDefault();
					if (modeRef.current !== "slice") {
						view.zoom(e.deltaY > 0 ? -0.08 : 0.08);
						return;
					}
					const info = dimsRef.current;
					const max = info ? info.dims[2] - 1 : 4096;
					sliceRef.current = Math.max(0, Math.min(max, sliceRef.current + (e.deltaY > 0 ? 1 : -1)));
					try {
						view.set_slice("axial", sliceRef.current);
					} catch (err) {
						/* the volume may not be loaded yet */
					}
					// The host measures on its own slice: keep it on the shown one.
					pushInput({ slice: sliceRef.current });
				};
				canvas.addEventListener("pointerdown", onDown);
				canvas.addEventListener("pointermove", onMove);
				canvas.addEventListener("pointerup", onUp);
				canvas.addEventListener("dblclick", onDblClick);
				canvas.addEventListener("contextmenu", onContextMenu);
				canvas.addEventListener("wheel", onWheel, { passive: false });
				return () => {
					canvas.removeEventListener("pointerdown", onDown);
					canvas.removeEventListener("pointermove", onMove);
					canvas.removeEventListener("pointerup", onUp);
					canvas.removeEventListener("dblclick", onDblClick);
					canvas.removeEventListener("contextmenu", onContextMenu);
					canvas.removeEventListener("wheel", onWheel);
				};
			}, [commitDraw, pushInput]);

			// The mode bar drives the renderer's layout.
			React.useEffect(() => {
				const view = viewRef.current;
				// No view before the module loaded and a study was opened: the effect
				// is inert until then, as the render-mode effect below is.
				if (!view) return;
				try {
					// By name, so an unknown value is reported here instead of
					// falling back to a 2D slice (F1).
					view.set_mode(mode);
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

			const changeOverlay = (patch) => {
				const next = Object.assign({}, overlayRef.current, patch);
				overlayRef.current = next;
				setOverlayShown(next.shown);
				setOverlayStyle(next.style);
				setOverlayFill(next.fill);
				const view = viewRef.current;
				if (view) {
					try {
						view.set_overlay(next.shown, next.fill, next.style);
					} catch (e) {
						/* the view may not be ready */
					}
				}
				pushInput({ overlays: { segments_shown: next.shown, fill_opacity: next.fill, style: next.style } });
			};

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
					button("MPR", mode === "mpr", () => { setMode("mpr"); pushInput({ component: "mpr" }); }),
					button("3D", mode === "volume", () => { setMode("volume"); pushInput({ component: "volume" }); }),
					button("2D", mode === "slice", () => { setMode("slice"); pushInput({ component: "slice" }); pushInput({ plane: "axial" }); }),
					h(
						"select",
						{ style: styles.select, value: renderMode, onChange: (e) => setRenderMode(e.target.value), "aria-label": "3D mode" },
						["tissue", "isosurface", "mip", "transfer_function"].map((m) =>
							h("option", { key: m, value: m, style: styles.option }, m)
						)
					),
					// The measurement tools (29.2-c): picking one switches to the 2D
					// view, where a drag draws on the slice and the host measures.
					h(
						"select",
						{
							style: styles.select, value: tool, "aria-label": "Measurement tool",
							onChange: (e) => {
								const next = e.target.value;
								setTool(next);
								draftRef.current = null;
								if (next !== "navigate") {
									setMode("slice");
									pushInput({ component: "slice" });
									pushInput({ tool: next });
								} else {
									pushInput({ tool: "pan" });
								}
							}
						},
						["navigate", "distance", "rect", "angle", "area"].map((t) =>
							h("option", { key: t, value: t, style: styles.option }, t)
						)
					),
					button("Overlay", overlayShown, () => changeOverlay({ shown: !overlayRef.current.shown })),
					h(
						"select",
						{ style: styles.select, value: overlayStyle, onChange: (e) => changeOverlay({ style: e.target.value }), "aria-label": "Overlay style" },
						["outline", "fill", "fill_outline"].map((s) => h("option", { key: s, value: s, style: styles.option }, s))
					),
					h("input", {
						type: "range", min: 0, max: 1, step: 0.05, value: overlayFill,
						"aria-label": "Overlay fill opacity", title: "Overlay fill opacity",
						onChange: (e) => changeOverlay({ fill: parseFloat(e.target.value) })
					})
				),
				h(
					"div",
					{ style: styles.stage },
					h("canvas", { ref: canvasRef, style: styles.canvas }),
					// The measurement overlay: committed values and the draft, above
					// the WebGPU canvas and below the MPR labels; it takes no pointer
					// events, so the canvas underneath keeps the drag.
					h("canvas", { ref: measureCanvasRef, style: styles.measureCanvas, "aria-hidden": "true" }),
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
				// The measured values as text, every slice: the overlay draws the
				// shown slice's shapes, this list names what the host measured.
				annotations.length > 0
					? h(
							"div", { style: styles.note },
							annotations
								.map((a) => a.name + " (" + a.plane + " " + a.slice_index + "): " +
									(a.value != null ? formatMeasure(a.value, a.unit) : a.label || ""))
								.join(" · ")
						)
					: null,
				// The TF editor is a render setting of the transfer_function mode
				// (panel-local, Stage 29 design §5): it appears once a view is
				// loaded and that mode is chosen.
				renderMode === "transfer_function" && viewRef.current ? h(TfEditor, { view: viewRef.current }) : null,
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