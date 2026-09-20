# Show available tasks.
default:
    @just --list

# Build the native macOS App without installing it.
build-macos:
    sh apps/macos/Scripts/build.sh

# Run the native macOS unit and contract tests.
test-macos:
    sh apps/macos/Scripts/promote-debug-test.sh
    sh apps/macos/Scripts/test.sh

# Run live tests through the running authenticated worktree Dev Instance.
test-macos-live:
    sh dev/dev-instance.sh test-live

# Verify the packaged native macOS App and embedded daemon.
test-macos-package:
    sh apps/macos/Scripts/test-runtime-package.sh

# Build, install, and open Clumsies.app for everyday use (Debug).
install-macos:
    sh apps/macos/Scripts/promote-debug.sh

# Compatibility alias for earlier installation instructions.
alias promote-debug-macos := install-macos

# Start the complete worktree-scoped Dev Instance.
dev-macos:
    sh dev/dev-instance.sh up

# Open a signed-in local Dev App with interactive Review scenarios.
dev-macos-reviews:
    sh dev/dev-instance.sh up --review-playground

# Show the current worktree Dev Instance status.
dev-macos-status:
    sh dev/dev-instance.sh status

# Show logs for the current worktree Dev Instance.
dev-macos-logs:
    sh dev/dev-instance.sh logs

# Stop the current worktree Dev Instance and preserve its data.
dev-macos-down:
    sh dev/dev-instance.sh down

# Delete the current worktree Dev Instance data and credentials.
dev-macos-reset:
    sh dev/dev-instance.sh reset

# Start the current worktree Dev Instance against a Preview descriptor.
dev-macos-preview descriptor:
    sh dev/dev-instance.sh up --preview "{{descriptor}}"

# Run the worktree Dev Instance lifecycle contract.
test-dev-macos:
    sh dev/dev-instance-test.sh
