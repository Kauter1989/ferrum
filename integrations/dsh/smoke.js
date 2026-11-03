#!/usr/bin/env node
/**
 * Drive a live FERRUM view host and check that it answers as `ferrum-view/1`
 * says it does (criterion 25.6-b).
 *
 *   node integrations/dsh/smoke.js http://127.0.0.1:41234 <token>
 *
 * It needs nothing but Node 18+ (the built-in fetch): start the host with
 * `ferrum-cli view serve --workspace <dir>` and pass the url and token it
 * prints. Exit code 0 means every check passed; 1 prints what failed.
 */
"use strict";

const [url, token] = process.argv.slice(2);

function fail(message) {
	console.error("smoke: " + message);
	process.exit(1);
}

function assert(condition, message) {
	if (!condition) fail(message);
}

async function main() {
	assert(url && token, "usage: node integrations/dsh/smoke.js <url> <token>");
	const base = url.replace(/\/+$/, "");
	const withToken = (path) => base + path + (path.includes("?") ? "&" : "?") + "token=" + encodeURIComponent(token);
	const post = async (body) => {
		const response = await fetch(withToken("/api/input"), {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify(body)
		});
		assert(response.ok, "POST " + JSON.stringify(body) + " answered " + response.status);
		return response.json();
	};

	// 1. The token is the only credential.
	const denied = await fetch(base + "/api/state");
	assert(denied.status === 401, "a request without the token answered " + denied.status + ", expected 401");

	// 2. The state document.
	const stateResponse = await fetch(withToken("/api/state"));
	assert(stateResponse.ok, "state answered " + stateResponse.status);
	const state = await stateResponse.json();
	assert(state.format === "ferrum-view/1", "state format is " + JSON.stringify(state.format));
	assert(["axial", "coronal", "sagittal"].includes(state.plane), "unknown plane " + JSON.stringify(state.plane));
	assert(Number.isInteger(state.slice_count) && state.slice_count > 0, "slice_count is " + state.slice_count);
	assert(state.slice_index >= 0 && state.slice_index < state.slice_count, "slice_index outside the volume");
	assert(state.window && state.window.width > 0, "the window has no width");
	assert(["HU", "raw"].includes(state.volume.value_unit), "value_unit is " + JSON.stringify(state.volume.value_unit));

	// 3. One frame at native resolution, as a PNG.
	const frameResponse = await fetch(withToken("/api/frame.png"));
	assert(frameResponse.ok, "frame answered " + frameResponse.status);
	assert(
		(frameResponse.headers.get("content-type") || "").includes("image/png"),
		"frame content type is " + frameResponse.headers.get("content-type")
	);
	const frame = Buffer.from(await frameResponse.arrayBuffer());
	assert(frame.length > 100, "the frame is only " + frame.length + " bytes");
	assert(frame.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])), "the frame is not a PNG");
	const width = frame.readUInt32BE(16);
	const height = frame.readUInt32BE(20);
	assert(width === state.volume.dims[0] || width === state.volume.dims[1] || width === state.volume.dims[2],
		"frame width " + width + " is not a plane of " + state.volume.dims);
	assert(height > 0, "the frame has no height");

	// 4. One wheel notch is one slice.
	const start = state.slice_index;
	const stepped = await post({ step: 1 });
	const expected = Math.min(start + 1, state.slice_count - 1);
	assert(stepped.slice_index === expected, "a step from " + start + " gave " + stepped.slice_index + ", expected " + expected);
	assert(stepped.rev > state.rev, "the revision did not increase");

	// 5. A pick gives a voxel, millimetres and a value.
	const picked = await post({ probe: { x: Math.floor(width / 2), y: Math.floor(height / 2) } });
	assert(picked.probe && Array.isArray(picked.probe.voxel) && picked.probe.voxel.length === 3, "the pick gave no voxel");
	assert(picked.probe.voxel[0] >= 0 && picked.probe.voxel[0] < state.volume.dims[0], "the picked column is outside");
	assert(Array.isArray(picked.probe.patient_mm) && picked.probe.patient_mm.length === 3, "the pick gave no millimetres");
	assert(Number.isFinite(picked.probe.value), "the pick gave no value");

	// 6. The event log tells what happened.
	const eventsResponse = await fetch(withToken("/api/events?since=0"));
	assert(eventsResponse.ok, "events answered " + eventsResponse.status);
	const log = await eventsResponse.json();
	const kinds = log.events.map((event) => event.kind);
	assert(kinds.includes("slice_changed"), "the log lost the slice change");
	assert(kinds.includes("point_picked"), "the log lost the pick");
	const sequences = log.events.map((event) => event.seq);
	assert(sequences.every((seq, i) => i === 0 || seq > sequences[i - 1]), "the sequence numbers are not increasing");

	// 7. The page is self-contained and needs the token.
	const page = await (await fetch(withToken("/"))).text();
	assert(page.includes("<script>"), "the page has no script");
	assert(!/(https?:)?\/\/(?!127\.0\.0\.1|localhost)/.test(page), "the page references another origin");

	console.log(
		"smoke: ok — " + state.plane + " " + state.slice_count + " slices, frame " + width + "×" + height +
		", " + log.events.length + " event(s), value " + picked.probe.value + " " + picked.probe.unit +
		" at voxel " + picked.probe.voxel.join(",")
	);
}

main().catch((error) => fail(error && error.message ? error.message : String(error)));
