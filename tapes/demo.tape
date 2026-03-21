# axon demo -- is my mac okay right now?
# Record: vhs demo.tape

Output tapes/demo.gif

Set Shell "zsh"
Set FontSize 14
Set Width 900
Set Height 340
Set Theme "Catppuccin Mocha"
Set Padding 20
Set TypingSpeed 40ms
Set WindowBar Colorful

# ── Setup (hidden) ────────────────────────────────────────────────────────────
Hide
Type "clear"
Enter
Sleep 500ms
Show

# ── Set the scene ─────────────────────────────────────────────────────────────
Type "# is my mac okay right now?"
Enter
Sleep 500ms

# ── One command answer ────────────────────────────────────────────────────────
Type "axon diagnose"
Enter
Sleep 6s

# ── Let the viewer read ───────────────────────────────────────────────────────
Sleep 2s

# ── Value prop punchline ──────────────────────────────────────────────────────
Type "# your ai agent checks this automatically, mid-session"
Enter
Sleep 3s
