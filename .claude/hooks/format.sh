#!/usr/bin/env bash
# Formats the file Claude just edited. Never blocks: formatting failures are ignored.
file=$(jq -r '.tool_input.file_path // empty' 2>/dev/null)
[ -z "$file" ] || [ ! -f "$file" ] && exit 0

case "$file" in
  *.rs)
    command -v rustfmt >/dev/null && rustfmt --edition 2021 "$file" >/dev/null 2>&1
    ;;
  *.ts|*.tsx|*.js|*.jsx|*.json|*.css|*.md)
    if [ -x "$CLAUDE_PROJECT_DIR/apps/desktop/node_modules/.bin/prettier" ]; then
      "$CLAUDE_PROJECT_DIR/apps/desktop/node_modules/.bin/prettier" --write "$file" >/dev/null 2>&1
    fi
    ;;
esac
exit 0
