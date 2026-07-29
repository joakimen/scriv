# The README demo, as code. Rendered by `make demo`, which builds the sandbox
# in demo/fixture.sh first — every repository, branch, and pull request below is
# fictional, and `gh` is a stub, so nothing here touches a real account.
#
# Keep this under ~25 seconds: it is a GIF that autoplays on the project page.

Output "docs/demo.gif"

Set Shell bash
Set FontSize 15
Set Width 1200
# Just tall enough for the longest list plus the commands above it. The picker
# fills the terminal, so extra height is dead space in the GIF.
Set Height 280
Set Padding 20
Set Theme "Catppuccin Mocha"
Set TypingSpeed 55ms
Set Framerate 24

# Enter the sandbox off-camera: fake HOME, stub gh, freshly built scriv.
Hide
Type "source target/demo-fixture/env.sh"
Enter
Sleep 1s
Show

# --- 1. Find a repository among many, without knowing where it lives ---------
Type 'cd "$(scriv repo pick)"'
Sleep 400ms
Enter
Sleep 1200ms

# Groups are colour-tagged; the preview shows what is going on in each repo.
Type "bill"
Sleep 2s
Enter
Sleep 1200ms

# --- 2. Check out a branch that only exists on the remote --------------------
Type "scriv branch checkout"
Sleep 400ms
Enter
Sleep 1500ms

# The current branch leads, then local branches, then remote-only ones, each
# newest first. Green is local+remote, yellow local-only, cyan remote-only. The
# preview shows who has been working on the highlighted branch.
#
# Three rows down is the remote-only branch, at the bottom of the list.
Down
Sleep 600ms
Down
Sleep 600ms
Down
Sleep 1500ms

# Picking the cyan one creates the local branch and sets its upstream.
Enter
Sleep 2500ms

# --- 3. Pull requests, coloured by state ------------------------------------
Type "scriv pr ls --status"
Sleep 400ms
Enter
Sleep 3s
