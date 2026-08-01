# The README demo, as code. Rendered by `make demo`, against the sandbox in
# demo/fixture.sh — everything below is fictional and `gh` is a stub.
#
# Keep this under ~25 seconds: it is a GIF that autoplays on the project page.

Output "docs/demo.gif"

Set Shell bash
Set FontSize 15
Set Width 1200
# The selector fills the terminal, so extra height is dead space in the GIF.
Set Height 280
Set Padding 20
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
Set Framerate 24

# Enter the sandbox off-camera: fake HOME, stub gh, freshly built scriv.
Hide
Type "source target/demo-fixture/env.sh"
Enter
Sleep 1s
Show

# --- 1. Check out a branch that only exists on the remote --------------------
Type "scriv branch checkout"
Sleep 350ms
Enter
Sleep 1200ms

# Three rows down is the remote-only branch, at the bottom of the list.
Down
Sleep 500ms
Down
Sleep 500ms
Down
Sleep 1200ms

Enter
Sleep 2s

# --- 2. Find a file where you are standing and open it -----------------------
# $EDITOR is cat in the sandbox, so the file lands on screen.
Type "scriv edit"
Sleep 350ms
Enter
Sleep 1200ms

Type "quota"
Sleep 1500ms
Enter
Sleep 2s

# --- 3. Pull requests, with their checks -------------------------------------
Type "scriv pr ls --status"
Sleep 350ms
Enter
Sleep 3s
