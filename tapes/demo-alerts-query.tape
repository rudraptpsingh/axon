# axon demo -- agent queries blame after alert
# Record: vhs demo-alerts-query.tape

Output tapes/demo-alerts-query.gif

Set Shell "zsh"
Set FontSize 13
Set Width 900
Set Height 500
Set Theme "Catppuccin Mocha"
Set Padding 20
Set TypingSpeed 40ms
Set WindowBar Colorful

# ── Hidden setup ──────────────────────────────────────────────────────────────
Hide
Type "exec 2>/dev/null"
Enter
Type "export _DEMO_HOME=$(mktemp -d) && export HOME=$_DEMO_HOME"
Enter
Sleep 300ms
Type "clear"
Enter
Sleep 500ms
Show

# ── Part 1: Agent queries blame ───────────────────────────────────────────────
Type "# agent asks axon: what triggered the alert?"
Enter
Sleep 1000ms

Type "axon query process_blame"
Enter
Sleep 6000ms

# ── Closer ────────────────────────────────────────────────────────────────────
Sleep 1000ms
Type "# structured blame + fix, ready for your agent to act on"
Enter
Sleep 4000ms

# ── Cleanup (hidden) ─────────────────────────────────────────────────────────
Hide
Type "rm -rf $_DEMO_HOME"
Enter
Sleep 200ms
