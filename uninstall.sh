#!/usr/bin/env bash
# Restore plugin-owned configuration, then unlink herdr-agent-quota.
#
# Usage:
#   ./uninstall.sh
#
# The configuration action is intentionally run before unlinking: Herdr owns
# the plugin state directory used to restore Claude/Agy statusLine backups.
set -euo pipefail

command -v herdr >/dev/null 2>&1 || {
  printf 'error: Herdr is not installed or not on PATH\n' >&2
  exit 1
}

if herdr plugin list 2>/dev/null | grep -q 'herdr-agent-quota'; then
  # An earlier interrupted uninstall may have disabled the plugin. Enable it
  # long enough for Herdr to provide the state directory to the restore action.
  herdr plugin enable herdr-agent-quota >/dev/null 2>&1 || true
  printf '%s\n' '→ restoring plugin-owned configuration'
  herdr plugin action invoke herdr-agent-quota.uninstall

  printf '%s\n' '→ disabling and unlinking the Herdr plugin'
  herdr plugin disable herdr-agent-quota || true
  herdr plugin unlink herdr-agent-quota
  printf '%s\n' 'Uninstalled and restored.'
else
  printf '%s\n' 'herdr-agent-quota is not linked; no configuration was changed.'
fi
