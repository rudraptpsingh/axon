# axon demo -- install, configure, use
# Record: vhs demo-setup.tape

Output tapes/demo-setup.gif

Set Shell "zsh"
Set FontSize 13
Set Width 900
Set Height 600
Set Theme "Catppuccin Mocha"
Set Padding 20
Set TypingSpeed 40ms
Set WindowBar Colorful

# ── Hidden setup: isolated HOME + silence all stderr for this session ─────────
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

# ── Part 1: Install ───────────────────────────────────────────────────────────
Type "brew install axon"
Enter
Sleep 500ms

Hide
Sleep 3s
Type "clear"
Enter
Sleep 500ms
Show

Sleep 1000ms
Type "axon --version"
Enter
Sleep 2s

# ── Part 2: Configure your agent ─────────────────────────────────────────────
Sleep 1500ms
Type "# one command to configure your agent"
Enter
Sleep 1000ms

Type "axon setup cursor"
Enter
Sleep 2500ms

# ── Part 3: All commands ──────────────────────────────────────────────────────
Sleep 1500ms
Type "# everything axon can do"
Enter
Sleep 1000ms

Type "axon --help"
Enter
Sleep 4s

# ── Part 4: One-shot diagnosis ────────────────────────────────────────────────
Sleep 1500ms
Type "# check your machine right now"
Enter
Sleep 1000ms

Type "axon diagnose"
Enter
Sleep 7s
Sleep 2s

# ── Part 5: What the agent sees via MCP ──────────────────────────────────────
Sleep 1500ms
Type "# what your ai agent sees via mcp:"
Enter
Sleep 1000ms

Type "axon query process_blame"
Enter
Sleep 6s
Sleep 2s

# ── Closer ────────────────────────────────────────────────────────────────────
Sleep 1500ms
Type "# zero cloud. all local. plug in once, forget it."
Enter
Sleep 4s

# ── Cleanup (hidden) ─────────────────────────────────────────────────────────
Hide
Type "rm -rf $_DEMO_HOME"
Enter
Sleep 200ms
