#!/bin/bash
# Create and verify a rollback point before one boot-change transaction.
set -euo pipefail
cd "$(dirname "$0")/.."
label=${1:?Usage: tools/vm-snapshot.sh purpose}
[[ "$label" =~ ^[a-z0-9-]+$ ]] || { echo "Use a lowercase purpose label" >&2; exit 2; }
vm_id='{caee9cb3-bac7-41e2-85f2-32b3a7369114}'
mkdir -p .test-artifacts/snapshots
stamp=$(date +%Y%m%d-%H%M%S)
record=".test-artifacts/snapshots/$stamp-$label.txt"
prlctl snapshot "$vm_id" --name "$stamp-$label" --description "Before BackupRestore boot change: $label" > "$record" 2>&1
cat "$record"
snapshot_id=$(sed -nE 's/.*(\{[0-9a-fA-F-]{36}\}).*/\1/p' "$record" | tail -1)
[[ -n "$snapshot_id" ]] || { echo "Snapshot ID missing; STOP" >&2; exit 1; }
prlctl snapshot-list "$vm_id" > "$record.verified"
rg -F -- "$snapshot_id" "$record.verified" >/dev/null || { echo "Snapshot verification failed; STOP" >&2; exit 1; }
echo "VERIFIED_SNAPSHOT=$snapshot_id"
