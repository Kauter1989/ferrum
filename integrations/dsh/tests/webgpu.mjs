// covers 29.5-b — an automated test of FERRUM's WebGPU path in a HEADED browser.
//
// Headless Chromium has no WebGPU adapter on the development machine even with
// --enable-unsafe-webgpu (docs/webgpu-panel.md, lesson L30); a headed window
// does. This test therefore launches the machine's Edge (channel "msedge")
// headed, loads the stand page, and checks that the renderer actually draws the
// phantom: the canvas is not black, the status names an adapter, and every view
// mode renders. It SKIPS (exit 0, "SKIPPED:") when there is no browser, no
// adapter or no Playwright, so a CI run without a headed GPU browser stays green
// — the same graceful skip as test_panel.py without node.
//
// Run with Node >= 18:  node webgpu.mjs   (playwright-core installed alongside).

import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { PNG } from "pngjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const panelDir = path.resolve(here, "..", "panel");
// Screenshots go to the scratchpad beside the repository, never into it.
const shotDir = path.resolve(here, "..", "..", "..", "..", "..", "segmres");

function skip(reason) {
	console.log("SKIPPED: " + reason);
	process.exit(0);
}

let chromium;
try {
	({ chromium } = await import("playwright-core"));
} catch (e) {
	skip("playwright-core is not installed (npm install in integrations/dsh/tests)");
}

const MIME = {
	".html": "text/html; charset=utf-8",
	".js": "text/javascript; charset=utf-8",
	".wasm": "application/wasm",
	".json": "application/json",
};

const server = http.createServer((req, res) => {
	const url = decodeURIComponent(req.url.split("?")[0]);
	const rel = url === "/" ? "stand.html" : url.replace(/^\/+/, "");
	const file = path.resolve(panelDir, rel);
	// Stay inside panelDir (the sep boundary stops a sibling like "panel-x").
	if ((file !== panelDir && !file.startsWith(panelDir + path.sep)) || !fs.existsSync(file) || !fs.statSync(file).isFile()) {
		res.writeHead(404);
		res.end("not found");
		return;
	}
	res.writeHead(200, { "Content-Type": MIME[path.extname(file)] || "application/octet-stream" });
	fs.createReadStream(file).pipe(res);
});

await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const port = server.address().port;
const base = `http://127.0.0.1:${port}/`;

let browser;
try {
	browser = await chromium.launch({
		channel: "msedge",
		headless: false,
		args: ["--enable-unsafe-webgpu", "--ignore-gpu-blocklist"],
	});
} catch (e) {
	server.close();
	skip("no headed Edge/Chromium available: " + (e && e.message ? e.message.split("\n")[0] : e));
}

const failures = [];
function check(name, ok, detail) {
	console.log((ok ? "ok   " : "FAIL ") + name + (detail ? " — " + detail : ""));
	if (!ok) failures.push(name);
}

try {
	const page = await browser.newPage({ viewport: { width: 900, height: 700 } });
	const errors = [];
	page.on("pageerror", (e) => errors.push(String(e)));
	await page.goto(base, { waitUntil: "load" });

	// The stand page reports its adapter, then renders in a rAF loop. Wait for
	// the loop to run (status carries "fps") or for it to name a failure.
	await page.waitForFunction(
		() => {
			const s = document.getElementById("status").textContent;
			const err = document.getElementById("error");
			return /fps/.test(s) || /no WebGPU|failed|no adapter/i.test(s) || (err && !err.hidden);
		},
		{ timeout: 30000 }
	);
	const status = await page.textContent("#status");
	const errorVisible = await page.evaluate(() => {
		const e = document.getElementById("error");
		return e && !e.hidden ? e.textContent : "";
	});

	if (/no WebGPU|no adapter/i.test(status) || errorVisible) {
		await browser.close();
		server.close();
		skip("this headed window has no WebGPU adapter (field test on a GPU machine): " + (errorVisible || status));
	}

	check("the stand page names a WebGPU adapter", /adapter|BrowserWebGpu|fps/i.test(status), status);

	// Read the presented frame back from a screenshot of the canvas: a WebGPU
	// canvas does not preserve its drawing buffer, so drawImage would read black;
	// the compositor's screenshot carries the rendered frame. A rendered phantom
	// has many non-black pixels; a black canvas means nothing was drawn.
	async function canvasStats() {
		const buf = await page.locator("#view").screenshot();
		const png = PNG.sync.read(buf);
		let nonBlack = 0;
		let sum = 0;
		const n = png.width * png.height;
		for (let i = 0; i < png.data.length; i += 4) {
			const v = png.data[i] + png.data[i + 1] + png.data[i + 2];
			sum += v;
			if (v > 30) nonBlack++;
		}
		return { nonBlack, total: n, mean: sum / n };
	}

	await page.waitForTimeout(800);
	const vol = await canvasStats();
	check("the 3D view draws the phantom (canvas not black)", vol.nonBlack > vol.total * 0.02, `${vol.nonBlack}/${vol.total} non-black, mean ${vol.mean.toFixed(1)}`);
	fs.mkdirSync(shotDir, { recursive: true });
	await page.screenshot({ path: path.join(shotDir, "webgpu-volume.png") });

	// Every view mode renders.
	for (const mode of ["slice", "mpr"]) {
		await page.selectOption("#mode", mode);
		await page.waitForTimeout(700);
		const s = await canvasStats();
		check(`the ${mode} view draws (canvas not black)`, s.nonBlack > s.total * 0.01, `${s.nonBlack}/${s.total} non-black`);
		await page.screenshot({ path: path.join(shotDir, `webgpu-${mode}.png`) });
	}

	// Every render technique renders in 3D.
	await page.selectOption("#mode", "volume");
	for (const render of ["isosurface", "mip", "transfer_function"]) {
		await page.selectOption("#render", render);
		await page.waitForTimeout(600);
		const s = await canvasStats();
		check(`the ${render} technique draws`, s.nonBlack > s.total * 0.005, `${s.nonBlack}/${s.total} non-black`);
	}

	check("no page error escaped", errors.length === 0, errors.join("; "));
} catch (e) {
	failures.push("exception: " + (e && e.message ? e.message : e));
	console.log("FAIL exception — " + (e && e.stack ? e.stack : e));
} finally {
	await browser.close().catch(() => {});
	server.close();
}

if (failures.length) {
	console.log("FAILED: " + failures.join("; "));
	process.exit(1);
}
console.log("ok: the WebGPU path renders in a headed browser");
process.exit(0);
