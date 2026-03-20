# VHS tape file for axon demo recording
# Install VHS: brew install charmbracelet/tap/vhs
# Record:      vhs demo.tape

Output demo.gif

Set Shell "zsh"
Set FontSize 16
Set Width 900
Set Height 500
Set Theme "Catppuccin Mocha"
Set Padding 20

# Title
Type "# axon -- local hardware intelligence for AI coding agents"
Enter
Sleep 1.5s

# Show help briefly
Type "axon --help"
Enter
Sleep 3s

# Run diagnose to show process blame
Type "# Let's see what's slowing this Mac down..."
Enter
Sleep 1s

Type "axon diagnose"
Enter
Sleep 6s

# Show hardware snapshot
Type "# Raw hardware state as JSON:"
Enter
Sleep 1s

Type "axon status"
Enter
Sleep 4s

# Simulate a runaway process
Type "# Now let's stress the CPU and watch axon catch it..."
Enter
Sleep 1s

Type "yes > /dev/null &"
Enter
Sleep 1s

Type "yes > /dev/null &"
Enter
Sleep 1s

Type "yes > /dev/null &"
Enter
Sleep 1s

Type "# Wait a few seconds for axon to detect the anomaly..."
Enter
Sleep 5s

Type "axon diagnose"
Enter
Sleep 6s

# Clean up
Type "# axon caught it. Cleaning up..."
Enter
Sleep 1s

Type "killall yes"
Enter
Sleep 1s

# Verify recovery
Type "axon diagnose"
Enter
Sleep 6s

Type "# Back to healthy. Zero cloud. Zero telemetry. Pure local."
Enter
Sleep 3s
