# axon demo -- detecting a runaway process
# Record: vhs demo-detect.tape

Output tapes/demo-detect.gif

Set Shell "zsh"
Set FontSize 15
Set Width 900
Set Height 600
Set Theme "Catppuccin Mocha"
Set Padding 20
Set TypingSpeed 30ms
Set WindowBar Colorful

# ── Setup: clear screen (MCP demo uses scripts/mcp-call.sh -- VHS parses ":" in Type strings) ──
Hide
Type "exec 2>/dev/null"
Enter
Type "clear"
Enter
Sleep 500ms
Show

# ── 1. Baseline: healthy system ──────────────────────────────────────────────
Type "# baseline: system is healthy"
Enter
Sleep 300ms

Type "axon diagnose"
Enter
Sleep 7s

# ── 2. Show intent, then spawn CPU hogs (hidden) ────────────────────────────
Type "# simulating a runaway process..."
Enter
Sleep 500ms

Type "for i in 1 2 3 4; do yes >/dev/null & done"
Enter

Hide
Sleep 500ms
Show

# Wait for EWMA to register sustained anomaly (3+ samples x 2s = 6s+)
Sleep 8s

# ── 3. axon catches it ──────────────────────────────────────────────────────
Type "axon diagnose"
Enter
Sleep 7s

# ── 4. What the AI agent sees via MCP ────────────────────────────────────────
Type "# what your AI agent sees via MCP:"
Enter
Sleep 300ms

Type "./scripts/mcp-call.sh process_blame"
Enter
Sleep 8s

# ── 5. Agent acts on the suggested fix ───────────────────────────────────────
Type "# agent reads the fix and acts on it"
Enter
Sleep 300ms

Hide
Type "killall yes 2>/dev/null"
Enter
Sleep 500ms
Show

Type "killall yes"
Enter
Sleep 1s

# ── 6. Verify recovery ──────────────────────────────────────────────────────
Type "axon diagnose"
Enter
Sleep 7s

Type "# detect -> blame -> fix -> verify"
Enter
Sleep 3s
