# axon demo -- configure webhook alerts
# Record: vhs demo-alerts-config.tape

Output tapes/demo-alerts-config.gif

Set Shell "zsh"
Set FontSize 13
Set Width 900
Set Height 550
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
Type "mkdir -p $HOME/.config/axon"
Enter
Type "cat > $HOME/.config/axon/alert-dispatch.json << 'EOF'"
Enter
Type '{'
Enter
Type '  "channels": ['
Enter
Type '    {'
Enter
Type '      "type": "webhook",'
Enter
Type '      "id": "myapp",'
Enter
Type '      "url": "https://yourapp.com/alerts",'
Enter
Type '      "filters": { "severity": [], "alert_types": ["*"] }'
Enter
Type '    }'
Enter
Type '  ]'
Enter
Type '}'
Enter
Type "EOF"
Enter
Sleep 300ms
Type "clear"
Enter
Sleep 500ms
Show

# ── Part 1: One-line flag ─────────────────────────────────────────────────────
Type "# send alerts to your app with one flag"
Enter
Sleep 1000ms

Type "axon serve --alert-webhook myapp=https://yourapp.com/alerts"
Enter
Sleep 2000ms

Hide
Type "C-c"
Sleep 500ms
Type "clear"
Enter
Sleep 300ms
Show

# ── Part 2: Config file ───────────────────────────────────────────────────────
Sleep 500ms
Type "# or use a config file for more control"
Enter
Sleep 1000ms

Type "cat ~/.config/axon/alert-dispatch.json"
Enter
Sleep 3500ms

# ── Closer ────────────────────────────────────────────────────────────────────
Sleep 1000ms
Type "# supports multiple channels, severity filters, alert type filters"
Enter
Sleep 4000ms

# ── Cleanup (hidden) ─────────────────────────────────────────────────────────
Hide
Type "rm -rf $_DEMO_HOME"
Enter
Sleep 200ms
