set dotenv-load

_default:
  @just --list

fix:
  #!/usr/bin/env bash
  # Fix markdown formatting with Prettier
  bunx prettier --write "**/*.md" --config ./.prettierrc

  git ls-files "*.md" | grep -v "docsite" | xargs -r bunx markdownlint-cli2 --fix


check:
  #!/usr/bin/env bash
  set -e

  # Check markdown
  bunx prettier -c "**/*.md" --config ./.prettierrc
  git ls-files "*.md" | grep -v "docsite" | xargs -r bunx markdownlint-cli2

