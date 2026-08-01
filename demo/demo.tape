# The README demo, as code. Rendered by `make demo`, which builds the sandbox
# in demo/fixture.sh first — every repository, branch, and pull request below is
# fictional, and `gh` is a stub, so nothing here touches a real account.
#
# Keep this under ~25 seconds: it is a GIF that autoplays on the project page.
#
# It opens on branch switching rather than on `cd "$(scriv repo sel)"`. That
# composition is worth knowing, but a subshell in the first frame asks the
# viewer to read shell syntax before they have seen what scriv does; the
# README's "Every `sel` prints one line" section is where it belongs.

Output "docs/demo.gif"

Set Shell bash
Set FontSize 15
Set Width 1200
# Just tall enough for the longest list plus the commands above it. The selector
# fills the terminal, so extra height is dead space in the GIF.
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

# The current branch leads, then local branches, then remote-only ones, each
# newest first. Green is local+remote, yellow local-only, cyan remote-only. The
# preview shows who has been working on the highlighted branch.
#
# Three rows down is the remote-only branch, at the bottom of the list.
Down
Sleep 500ms
Down
Sleep 500ms
Down
Sleep 1200ms

# Selecting the cyan one creates the local branch and sets its upstream.
Enter
Sleep 2s

# --- 2. Find a file where you are standing and open it -----------------------
# The walk honours .gitignore and streams into the selector; the preview is the
# file itself. $EDITOR is cat in the sandbox, so the file lands on screen.
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
