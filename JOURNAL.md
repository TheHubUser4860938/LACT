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
  - RESULT 2026-10-21 (owner's run, root daemon + fork GUI): the tool works
    on AD102 + driver 615.78.08.
    - XBAR offset +225 MHz applied; effective ~2700 MHz under load (full
      adoption at this offset — unlike Blackwell's power-capped partials).
    - Game data (Control: Resonance, single runs, not yet A/B/A):
      122 fps stock -> 126-128 fps at XBAR +225 (~+3.3 to +4.9%). Ada responds.
    - SYS: follows XBAR on its own (matches the RTX 2000 Ada observation on
      the PR and the propagation model); adding an offset to SYS on top of
      XBAR = unstable. Do not double-tune propagation-linked domains.
    - MSVDD: write fails on this card/branch. Error decoded 2026-10-21 from
      NVIDIA's nvstatuscodes.h (open-gpu-kernel-modules, fetched): status
      0x2c = NV_ERR_INVALID_INDEX. Frequency-offset writes on the same entry
      work (owner runs XBAR +225 live); only the rail-voltage field is
      rejected. Interpretation (inference, labeled): on Ada the per-domain
      rail-arbitration mapping has no entry — the MSVDD control layer is
      Blackwell-only in practice; frequency offsets are per-domain, rail
      offsets are not. mVolt+ on Ada does voltage per-pstate via a different
      control, consistent with this. First AD102 data point for #1158/#1147.
    - Owner-run result 2026-10-21: XBAR +225 stable and adopted (effective
      ~2700 MHz under load); Control: Resonance 122 -> 126-128 fps (+3.3-4.9%,
      single runs). SYS follows XBAR; direct SYS offset on top = unstable
      (matches propagation model — do not double-tune dependent domains).
  - CONFIRMED 2026-10-21 (owner's first run): the GET_INFO read path works on
    AD102 + driver 615.78.08 — the fork GUI showed the XBAR and other domain
    rows on this card. First such data point (upstream only ever probed
    GB202/GB203 + RTX 2000 Ada).
  - CONFIRMED 2026-10-21: writes from an unprivileged RM client fail with
    status 0x1b (NV_ERR_INSUFFICIENT_PERMISSIONS), exactly as documented on
    #1159. Root daemon required; GUI standalone = user = 0x1b.
  - CORRECTION 2026-10-21: there is no separate lact-daemon binary — the
    workspace builds one `lact` binary and the daemon is `lact daemon`
    (same as the systemd ExecStart). Handed corrected commands to owner.
  - PENDING: rows + ranges on the 4090 from the root-daemon run; then first
    write (small offset, exact read-back, restore).
  - HANDOFF 2026-10-21: probe built and committed; unprivileged run confirmed
    the permission wall (RM ioctl EINVAL/0x1b for non-root — matches #1159's
    finding). Handed owner the two commands: read dump first; the +10 mV
    write test on XBAR only if the read shows nothing new (owner's rule).
    Write test restores the full preimage — XBAR +225 passes through intact.
  - IMPLEMENTED 2026-10-21 (per owner's instruction: read first; write test
    only if the read shows nothing new; layout verified before writing):
    new debug bin lact-daemon/src/bin/clkdom_probe.rs (auto-detected bin
    target). Read mode: GET_INFO (per-entry object type + ranges, with raw
    audit bytes at both the 0x10 and the 0x0b range offsets), GET_CONTROL
    (rail fields), CLK_MEASURE_FREQ per active domain, PROP_RELS info+control
    (type-3 ratio question for AD102). Write mode (--write-test <domain> uV,
    opt-in): daemon's own discipline — full preimage, one field, SET,
    readback, full restore, exact-restore check, physical clock before/after.
    Layout cross-checked three ways: the daemon struct matches Loong0x00's
    hardware-verified layout on #1147 byte for byte (control base 0x3c,
    stride 0x40, MSVDD field at entry+0x14); the daemon's compile-time size
    asserts match the threads' measured sizes (0x3030/0x83c); and the same
    control block already applies XBAR +225 on this box for frequency, so
    the struct layout is proven correct here and the rail field is the only
    open question.
  - FIX 2026-10-21 (owner ran it): REGISTER_FD panicked EINVAL — my bug, the
    call was inverted. The daemon issues the ioctl on the DEVICE fd with the
    control fd as the argument (driver.rs); I had it the other way. Polished
    to mirror the daemon exactly, rebuilt clean. Root was never the issue at
    that line — re-run handed back to owner.
- P2: propagation ratio probe (read-only first — layout proven on GB202
  only); per-domain physical-clock display via CLK_MEASURE_FREQ.
- P3: domain V/F display (already in stack); #1218 implementation.
- P4: A/B/A bench — XBAR-sensitive synthetic (FurMark class) + one or two
  games, per-domain offset sweep, Xid watch, L2 checker; post the
  Ada + R615 data to the upstream issues.
- P5 (bench results first, no promises): what Ada headroom is worth.
  Published gains so far are Blackwell numbers only (~3% in games, ~10% in
  XBAR-heavy synthetics, +9.3% via ratio on FurMark — all GB202).
