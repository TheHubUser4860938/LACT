# Journal — feat/mvolt-parity

Project journal per owner's multi-window rule: decisions land here, dated, in
the window they are made. Never erase, annotate.

## 2026-10-21 — setup (owner's decisions, made in the kickoff window)

Goal: a personal LACT fork with mVolt+ parity features for an RTX 4090
(Ada / AD102) on Linux — XBAR/SYS clock offsets with per-domain MSVDD, the
GPC propagation ratio, per-domain V/F curve display, full-range VRAM offset,
and per-domain physical-clock readouts.

Decisions:
- v1 scope = the full parity panel (owner's pick).
- Personal fork; measurements get reported back to the upstream issues.
  No bet on a fast merge: the LACT owner closed PR #1158 on 2026-09-25
  saying he wants to test the NvAPI route himself first.

Measured baseline (this machine, 2026-10-21):
- RTX 4090 Strix OC, driver 615.78.08, open kernel module.
- LACT 0.10.1 installed and running (0.10.1 already shows the extra clock
  domains read-only — the closed PR is the write path).
- CachyOS, kernel 7.2.10-1, gtk4 4.24.1, libadwaita 1.10.0, rust 1.99.1.
- License: MIT upstream. Toolchain pins channel 1.97.0; local 1.99.1 is
  newer, builds fine in practice.

Stack (Panchovix's branches, rebased onto upstream/master 16af419c —
rebase was clean, 6 commits, zero conflicts):
- feat/nvidia-clock-domain-offsets → PR #1158 (closed): XBAR/SYS/Power
  frequency offsets + per-domain MSVDD, via RM ioctl client
  (CLK_CLK_DOMAINS_GET_INFO / GET_CONTROL / SET_CONTROL), Turing and newer.
  Layouts: Blackwell entry type 0x10, pre-Blackwell 0x0b; the old layout was
  probed on an RTX 2000 Ada, which did respond (+300 MHz XBAR moved the core;
  SYS followed). No AD102 + R615 result published anywhere yet — that's ours.
- feat/nvidia-xbar-ratio → PR #1162 (closed): GPC→XBAR/SYS/Power/Legacy
  propagation ratio (U16.16, full-preimage restore).
- feat/nvidia-domain-vf-curves: display the domain V/F curves on MSVDD.
- NOT merged: Loong0x00/blackwell-multidomain-vf — author asked that it not
  be merged; reference for the per-domain V/F data model (127-point banks,
  per-point writes work only from a privileged RM client; PWR bank accepts
  SET but does not adopt — readback ≠ adoption).
- Not yet implemented: #1218 — VRAM offset beyond the NVML-advertised range
  via the RM clock-domain control. First net-new code in this fork.

## Guardrails (from the source threads — all fetched 2026-10-21)

- RM layouts are private and driver-branch-specific; derived on R610. Ours is
  615.78.08. Rule: read-only GET_INFO probe on AD102, validate returned
  structures, only then short SET / read-back / restore.
- Offsets shift the domain's V/F curve; they do nothing at idle or under
  locked clocks. Measure only under real load.
- MSVDD has no physical read path. ±50 mV is a guard rail; a positive offset
  lowers the domain's demand; a bad value corrupts memory traffic rather than
  failing cleanly.
- A finite memory-clock maximum silently constrains XBAR/SYS
  (NVIDIA/open-gpu-kernel-modules#1266). Do not lock MCLK; keep strict
  single-domain ranges off until the bench phase.
- Readback is not stability: watch Xids and visual corruption; use Loong0x00's
  L2 integrity checker (issue #1147 comment of 2026-08-16).
- Snapshot-test trap: tests/snapshots break on machines with a real GPU at
  0000:09:00.0 — never regenerate the nvidia snapshot on this box.

## Sources

- https://github.com/ilya-zlobintsev/LACT/issues/1147 (discovery, C demo,
  layouts, A/B data, integrity check)
- https://github.com/ilya-zlobintsev/LACT/pull/1158 (the base of this stack)
- https://github.com/ilya-zlobintsev/LACT/pull/1162 (ratio)
- https://github.com/ilya-zlobintsev/LACT/issues/1159 (clock topology,
  CLK_MEASURE_FREQ, strict ranges, V/F banks)
- https://github.com/ilya-zlobintsev/LACT/issues/1218 (VRAM range)
- https://github.com/SHANAjam/rtx5090-xbar-control (Windows/NvAPI confirmation)
- mVolt+ repo (closed source — README/docs only; credited Loong0x00 for the
  XBAR discovery, so no code flows from it; it stays the UX reference)

## Phases

- P0 (done 2026-10-21): clone, remotes (upstream / panchovix / loong0x00),
  stack rebase onto upstream/master — clean, 6 commits.
- P1: cargo check/build green (measured 2026-10-21: check 28.6 s, build
  36.7 s). Debug binaries ready. Daemon swap needs the owner's sudo
  (password prompt; agent cannot). Sequence given to owner; probe = fork
  daemon + fork GUI, look for domain rows with non-zero ranges on AD102
  driver 615.78.08 via the PR's type-checked 0x0b decode. Read-only first;
  writes (the PR's ignored round-trip test, LACT_TEST_GPU_MINOR) only after
  the read picture is clean. Note: the 4090 carries the live inference
  server (23.8/24.6 GB VRAM, 270 W) — fine for reads, pause the load for
  write tests and benches.
  - RESULT (pending): which domains report non-zero ranges on AD102/615:
  - CORRECTION 2026-10-21: there is no separate lact-daemon binary — the
    workspace builds one `lact` binary and the daemon is `lact daemon`
    (same as the systemd ExecStart). Handed corrected commands to owner.
- P2: propagation ratio probe (read-only first — layout proven on GB202
  only); per-domain physical-clock display via CLK_MEASURE_FREQ.
- P3: domain V/F display (already in stack); #1218 implementation.
- P4: A/B/A bench — XBAR-sensitive synthetic (FurMark class) + one or two
  games, per-domain offset sweep, Xid watch, L2 checker; post the
  Ada + R615 data to the upstream issues.
- P5 (bench results first, no promises): what Ada headroom is worth.
  Published gains so far are Blackwell numbers only (~3% in games, ~10% in
  XBAR-heavy synthetics, +9.3% via ratio on FurMark — all GB202).
