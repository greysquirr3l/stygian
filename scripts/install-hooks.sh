#!/usr/bin/env bash
# Install git hooks for stygian development.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GIT_DIR="$(git rev-parse --git-dir)"

echo "Installing git hooks..."

install_hook() {
  local hook_name="$1"
  local src="$SCRIPT_DIR/$hook_name"
  local dst="$GIT_DIR/hooks/$hook_name"

  if [[ ! -f "$src" ]]; then
    echo "  - $hook_name not found in scripts/, skipping"
    return 0
  fi

  cp "$src" "$dst"
  chmod +x "$dst"
  echo "  ✓ $hook_name"
}

install_hook pre-commit
install_hook pre-commit-gitleaks
install_hook pre-push
install_hook commit-msg

echo
echo "Git hooks installed successfully!"
echo
echo "Installed hooks run:"
echo "  • pre-commit: cargo fmt + dedicated gitleaks hook + cargo-audit"
echo "  • pre-commit-gitleaks: staged secret scan"
echo "  • pre-push: workspace test + strict clippy + release build + docs"
echo
echo "To bypass hooks (not recommended):"
echo "  git commit --no-verify"
echo "  git push --no-verify"
