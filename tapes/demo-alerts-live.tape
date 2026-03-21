# axon demo -- live alert firing
# Record: vhs demo-alerts-live.tape

Output tapes/demo-alerts-live.gif

Set Shell "zsh"
Set FontSize 13
Set Width 900
Set Height 600
Set Theme "Catppuccin Mocha"
Set Padding 20
Set TypingSpeed 40ms
Set WindowBar Colorful

# ── Hidden setup: start receiver + axon serve with MCP keepalive ──────────────
Hide
Type "exec 2>/dev/null"
Enter
Type "export _DEMO_HOME=$(mktemp -d) && export HOME=$_DEMO_HOME"
Enter
Sleep 300ms
Type "export _ALERT_FILE=/tmp/axon_demo_alert.json && rm -f $_ALERT_FILE"
Enter
Type "python3 /Users/rp/github/mcp-station/scripts/alert_receiver_minimal.py --output-file $_ALERT_FILE > /tmp/axon_rcv.log 2>&1 &"
Enter
Sleep 1500ms
Type "export _WEBHOOK_URL=$(grep -o 'http://[^ ]*' /tmp/axon_rcv.log)"
Enter
Sleep 300ms
Type "bash /Users/rp/github/mcp-station/scripts/demo_mcp_init.sh"
Enter
Sleep 300ms
Type "{ ( cat /tmp/axon_init.txt; sleep 20 ) | AXON_TEST_PREV_RAM_PRESSURE=normal AXON_TEST_PRESERVE_PREV_DURING_WARMUP=1 axon serve --alert-webhook demo=$_WEBHOOK_URL >/dev/null; } &disown"
Enter
Sleep 2000ms
Type "clear"
Enter
Sleep 500ms
Show

# ── Part 1: Set the scene ─────────────────────────────────────────────────────
Type "# axon is running in the background, watching your machine"
Enter
Sleep 2000ms

# ── Part 2: RAM spike ─────────────────────────────────────────────────────────
Type "# RAM pressure just spiked..."
Enter

# Wait for tick 4 to fire (~10s after serve starts)
Type "# waiting for alert to fire..."
Enter
Sleep 11000ms

# ── Part 3: Alert received ────────────────────────────────────────────────────
Type "# alert received:"
Enter
Sleep 500ms

Type "cat /tmp/axon_demo_alert.json"
Enter
Sleep 4000ms

# ── Part 4: Agent queries full context ───────────────────────────────────────
Sleep 500ms
Type "# agent queries full context:"
Enter
Sleep 800ms

Type "axon query process_blame"
Enter
Sleep 6000ms

# ── Closer ────────────────────────────────────────────────────────────────────
Sleep 500ms
Type "# proactive. local. no cloud."
Enter
Sleep 4000ms

# ── Cleanup (hidden) ─────────────────────────────────────────────────────────
Hide
Type "pkill -f alert_receiver_minimal 2>/dev/null; rm -rf $_DEMO_HOME /tmp/axon_demo_alert.json /tmp/axon_init.txt /tmp/axon_rcv.log"
Enter
Sleep 200ms
