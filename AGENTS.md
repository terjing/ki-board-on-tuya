# AGENTS.md

Operating contract for AI agents working in this repository. Human-authored
rules in `.kiro/steering/` and the user's direct instructions override anything
here. Read this file before making changes.

## What this project is

An **agentic macro-keyboard for Kiro**, built on the **Tuya T5AI-Core** board.
It gives Kiro CLI/IDE two things a normal keyboard cannot: **voice input piped
straight into Kiro** and **on-screen Agent status**. See `README.md` for the
phased roadmap (Phase 0 → 3) and current status.

It is a **two-stack** codebase:

- **Firmware** (`firmware/`) — TuyaOpen C for the T5-E1 module (Cortex-M33F,
  Wi-Fi6 + BLE 5.4). Built with the native `tos.py` CLI. Pluggable transport:
  UART (Phase 1) → BLE GATT (Phase 2) → BLE HID (Phase 3).
- **Companion** (`companion/`) — a Tauri app (Rust backend + React/TS frontend)
  that parses the wire protocol and injects keystrokes / drives ASR / reflects
  Kiro Agent status back to the board.

The two stacks talk over a shared **JSON-Lines wire protocol**; its single
source of truth is `shared/protocol/protocol.md` (`docs/protocol.md` is only a
reader's guide — never edit protocol semantics there).

## Repository layout

| Path | What lives there |
|------|------------------|
| `firmware/app/` | Product firmware skeleton (one app, build-time-selected transport/ASR variants) |
| `firmware/bringup/` | Standalone per-peripheral hardware validation apps (throwaway) |
| `firmware/feature-probes/` | Single-feature probes stepping toward the MVP demo (FP-1..FP-7) |
| `companion/` | Tauri companion app (`src/` = React/TS, `src-tauri/` = Rust) |
| `hardware/pinmap/` | **Authoritative** GPIO assignment, netlist, BOM, conflict checker |
| `shared/protocol/` | **Authoritative** wire-protocol spec |
| `docs/` | Design / spec / retrospective docs (Chinese-first) |
| `.kiro/specs/` | Per-feature spec folders (requirements/design/tasks) |
| `.kiro/steering/` | Human-authored steering — authoritative context, read it |
| `scripts/` | Windows (PowerShell) flash/debug helpers |
| `enclosure/`, `hardware/eda/` | Mechanical + EDA sub-projects |

`ref_projects/` is git-ignored **references only** — never treat it as part of
this repo and never copy pin definitions or protocol from it.

## Git rules (hard constraints)

- **Work only on a branch. Never commit, push, or force-push to `main`.**
  Create a feature branch (`feat/...`, `fix/...`, `chore/...`) for every change.
- **Pushing is limited to your own feature branch**, named explicitly:
  `git push -u origin <feature-branch>`. Never a bare `git push`, never
  `HEAD`/`@`, never `--all`/`--mirror`.
- You MAY open a pull request. You may **not** merge it — a human merges.
- You MAY commit freely on your own feature branch. **Never commit to `main`**
  without explicit permission from the user for that specific commit.
- Stage specific files, not `git add .`.
- Remote is `git@github.com:terjing/ki-board-on-tuya.git`.

## Secrets — never commit

- Device **licenses** (UUID / AuthKey) are per-device secrets. Never commit them,
  never print their values, never bake them into firmware in the repo.
- `scripts/*.ps1` reference local secrets and licensed flash flows — do not
  hardcode credentials into them.
- Flag any `.env`, key, or credential file before staging it.

## Build & test commands

Do **not** assume you can build or flash firmware locally. Firmware is built on
the **EC2 devbox** and flashed from **Windows**. In this agent environment,
prefer the host-side checks that need no board or SDK.

### Firmware (TuyaOpen C)

```bash
# Build (EC2 devbox only — needs the TuyaOpen SDK)
source /data/projects/TuyaOpen/export.sh
cd firmware/app                       # or firmware/bringup/<app>, firmware/feature-probes/<probe>
tos.py config choice -c TUYA_T5AI_CORE.config
tos.py clean -f && tos.py build       # clean -f is REQUIRED after any .config change

# Host unit tests (no SDK, no board — run these to verify pure logic)
firmware/app/test/run_host_tests.sh   # ak_input_fsm + ak_protocol (gcc-compiled)

# Build-all + format/IO static checks for bring-ups
firmware/bringup/build_all.sh         # optionally: build_all.sh <app> <app>
python3 hardware/pinmap/check_pins.py # pin-conflict checker
```

- Pin definitions have ONE source of truth: `hardware/pinmap/pinmap.json` +
  `firmware/.../common/include/*board_io.h`. Do not duplicate or guess pins.
- The device connects to **2.4 GHz Wi-Fi only**. USB is Host-mode only (no USB
  HID); direct-connect goes over **BLE HID**.

### Companion (Tauri — Rust + React/TS)

```bash
cd companion
npm ci                                # Node 22+ required
npm run typecheck                     # tsc --noEmit — fast, no toolchain beyond node
cd src-tauri && cargo test            # protocol/transport/injection unit tests
# npm run tauri dev / build need the full Tauri toolchain (GUI target)
```

**Always run the relevant checks before presenting work:** firmware changes →
host tests + `check_pins.py` (+ EC2 build if available); companion changes →
`npm run typecheck` + `cargo test`. If you cannot run a check in this
environment, say so explicitly rather than claiming it passed.

## Versioning — do not fight the hook

`firmware/app/VERSION` is the single source of truth. A **pre-commit hook**
(`git config core.hooksPath .githooks`) auto-syncs and stages `VERSION` plus
every TuyaOpen profile when a commit touches `firmware/app` implementation.
Let it run; don't `--no-verify` it, and don't hand-edit profile versions.
Use `python3 firmware/app/tools/version.py {show,bump,set,check}` when a manual
version change is intended.

## Conventions

- **Docs are being converted to English.** Write new docs in English; when you
  edit a Chinese doc, you may translate it to English. Keep technical terms,
  identifiers, pin names and protocol field names exactly as they are.
- **Code comments and commit messages: English.**
- Match existing patterns in the file/stack you are editing. Pure logic modules
  get host unit tests (`run_host_tests.sh` pattern).
- Build artifacts (`dist/`, `*.bin`) are git-ignored — never commit them.

## Clarify before building

This project runs on explicit specs (`.kiro/specs/`). If a feature request has
gaps or ambiguity, **send it back and ask** before building — do not invent
requirements or assume scope.

## Subagent workflow

Use subagents to do the work. Three model-locked agents are available; assign by
role, not by task label:

- **`opus`** — the coordinator and thinker. Plans features, decomposes work,
  reviews plans and results against requirements, makes judgement calls, and
  resolves ambiguity. Keep the heavy reasoning here.
- **`sonnet`** — the general worker. Implements the plan: writes firmware/C,
  Rust/TS, tests, and docs. The default builder.
- **`haiku`** — the grunt worker. Mechanical, well-specified tasks: renames,
  file moves, boilerplate, running a known check, collecting output, formatting.

### Flow (run once for code, once for tests)

1. **Plan** — `opus` writes the plan from the request + relevant spec.
2. **Check the plan** — a separate reviewer (`opus`) validates the plan against
   requirements before any code is written.
3. **Build** — `sonnet` implements against the approved plan.
4. **Check the build** — a separate `sonnet` validates the implementation
   against the plan (adversarial review, didn't write the code).
5. **Done.**

Apply the same plan → check → build → check loop a second time for tests.

**Review cap:** each check step gets at most **3 rounds** (check → fix →
re-check). If it still fails after round 3, stop and report back to the user
with the open findings rather than looping further.

### Subagent operating notes

- Dispatch independent tasks in ONE parallel batch; serialize anything that
  shares a writer or a running service. Never hand the whole request to a single
  worker just to relay its answer.
- Give each worker a self-contained brief: goal, the spec/paths it needs, which
  files it owns, verifiable output, and its stop condition.
- A worker inherits the git rules above — branch only, never `main`.
- Validate real outcomes (build/test results, file diffs) before declaring done;
  a worker's success claim is not proof.
