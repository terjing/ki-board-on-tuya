// Unit tests for the host-platform UI labels (companion/src/platform.ts).
//
// Run from the repo root:  node --test companion/tools/*.test.mjs
// Requires a Node that strips TypeScript types by default (22.18+). This file
// lives outside tsconfig.json `include: ["src"]`, so it is not type-checked or
// bundled.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const {
  parsePlatform,
  uiLabels,
  MAC_DICTATION_TRIGGERS,
} = await import(new URL("../src/platform.ts", import.meta.url));

test("windows labels equal the pre-port literals", () => {
  const l = uiLabels("windows");
  assert.equal(l.jumpChat("N"), "Alt+N");
  assert.equal(l.jumpChat(3), "Alt+3");
  assert.equal(l.jumpChatRange, "Alt+1\u20139");
  assert.equal(l.commandBar, "Command bar (Ctrl+K)");
  assert.equal(l.dictation, "Windows dictation (Win+H)");
  assert.equal(l.hookPlaceholder, '"<path-to-companion.exe>" --hook');
  assert.equal(l.fileManager, "Explorer");
});

test("other and unknown platforms reuse the windows labels", () => {
  const win = uiLabels("windows");
  const other = uiLabels("other");
  for (const [k, v] of Object.entries(win)) {
    if (typeof v === "string") assert.equal(other[k], v, k);
  }
  assert.equal(other.jumpChat(5), win.jumpChat(5));

  assert.equal(parsePlatform(undefined), "other");
  assert.equal(parsePlatform(""), "other");
  assert.equal(parsePlatform("linux"), "other");
  assert.equal(parsePlatform(" Windows "), "windows");
  assert.equal(parsePlatform("MacOS"), "macos");
});

test("macos labels", () => {
  const l = uiLabels("macos");
  assert.equal(l.jumpChat(3), "\u23033");
  assert.equal(l.jumpChatRange, "\u23031\u20139");
  assert.equal(l.commandBar, "Command bar (\u2318K)");
  assert.equal(l.dictation, "macOS Dictation");
  assert.equal(l.fileManager, "Finder");
  assert.ok(l.hookPlaceholder.startsWith("'"), l.hookPlaceholder);
  assert.ok(l.hookPlaceholder.endsWith("' --hook"), l.hookPlaceholder);
});

test("dictation trigger values match the Rust wire names", () => {
  assert.deepEqual(
    MAC_DICTATION_TRIGGERS.map((t) => t.value),
    ["edit-menu", "double-control", "double-right-command", "double-left-command"],
  );
  const rust = readFileSync(
    new URL("../src-tauri/src/pc_asr.rs", import.meta.url),
    "utf8",
  );
  for (const { value } of MAC_DICTATION_TRIGGERS) {
    assert.ok(rust.includes(`"${value}"`), `pc_asr.rs lacks "${value}"`);
  }
});
