# Launch Plan

## Show HN

Title: `Show HN: Axon -- Local hardware intelligence for AI coding agents (macOS)`

Body: See POST.md (ready to paste)

Post to: https://news.ycombinator.com/submit

Timing: Weekday, 8-10am ET for best visibility.

---

## Twitter Thread

**Tweet 1 (hook)**

Your AI coding agent can write code, run tests, search files.

But it has no idea your Mac is about to melt.

I built axon -- an MCP server that gives AI agents real-time hardware awareness. Zero cloud. Zero telemetry.

Open source: github.com/rudraptpsingh/axon

**Tweet 2 (the problem)**

The pattern:
- Deep in a coding session with Claude/Cursor
- Agent fires off builds and edits
- Fan spins up, everything lags
- Force-quit, restart, lose context

The agent never knew it was killing your machine. It had no way to know.

**Tweet 3 (what it does)**

axon runs locally on your Mac and exposes 4 MCP tools:

- process_blame: finds the culprit process + suggests a fix
- hw_snapshot: CPU, temp, RAM, pressure, throttling
- battery_status: percentage, charging, time remaining
- system_profile: chip, cores, RAM, macOS version

**Tweet 4 (the hero tool)**

The star is process_blame.

It uses per-process EWMA baselines to detect anomalous spikes (not just raw usage). A persistence filter requires 3+ consecutive anomalous samples before alerting.

Result: no false positives from transient cargo builds.

**Tweet 5 (privacy)**

axon makes zero network calls. No telemetry. No analytics. No cloud.

This isn't a policy -- it's architecture. There is no networking code in the binary. Data never leaves your machine.

**Tweet 6 (install)**

Install in 10 seconds:

brew tap rudraptpsingh/axon
brew install axon

Restart Claude Desktop / Cursor / VS Code. axon auto-configures itself. That's it.

**Tweet 7 (CTA)**

Built this because I kept losing context when my 8GB Air choked during long sessions.

If you use AI coding agents on macOS, give it a try. MIT licensed.

github.com/rudraptpsingh/axon

Feedback welcome -- especially edge cases I haven't hit yet.

---

## Launch Checklist

- [ ] Cut release tag `v0.1.0` and verify GitHub Actions produces binaries
- [ ] Run `./update-formula.sh v0.1.0` in homebrew-axon, push to GitHub
- [ ] Test `brew tap rudraptpsingh/axon && brew install axon` on a clean machine
- [ ] Record demo: `vhs demo.tape` (requires: `brew install charmbracelet/tap/vhs`)
- [ ] Add demo.gif to README
- [ ] Post Show HN (weekday, 8-10am ET)
- [ ] Post Twitter thread
- [ ] Monitor HN comments for first 2 hours, reply to questions
