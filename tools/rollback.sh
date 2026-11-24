#!/usr/bin/env bash
# One-shot autonomous rollback for the ZENT75-HE (GOAL Phase-0 step 5):
#   1. flash the vendor stock image (default firmware/0.0.18.uf2) via the UF2 bootloader
#   2. restore the most recent config backup (backups/<latest>/) and verify every region
#   3. leave the board in application mode on stock firmware with the user's config
#
# Everything is logged to backups/<latest>/rollback_<ts>.log and echoed to the console.
# usage: tools/rollback.sh [path/to/image.uf2] [path/to/backups/<ts>]
set -uo pipefail
cd "$(dirname "$0")/.."

IMAGE="${1:-firmware/0.0.18.uf2}"
BACKUP="${2:-$(ls -1d backups/*/ 2>/dev/null | sort | tail -1)}"
EXPECT="$(basename "$IMAGE" .uf2)"   # e.g. 0.0.18

if [ -z "${BACKUP:-}" ] || [ ! -f "${BACKUP%/}/dump.json" ]; then
  echo "no backup dump found (looked in '${BACKUP:-}'); run tools/dump_all.py first" >&2
  exit 1
fi
BACKUP="${BACKUP%/}"
TS=$(date +%s)
LOG="$BACKUP/rollback_${TS}.log"

{
  echo "=== ZENT75-HE rollback $(date -Is) ==="
  echo "image:  $IMAGE  (expect fw $EXPECT)"
  echo "backup: $BACKUP"
  echo
  echo "--- step 1/2: flash stock firmware via bootloader ---"
  python3 tools/flash_uf2.py "$IMAGE" --expect "$EXPECT" || { echo "FLASH FAILED"; exit 2; }
  echo
  echo "--- step 2/2: restore config backup and verify ---"
  python3 tools/restore.py "$BACKUP" || { echo "RESTORE/VERIFY FAILED"; exit 3; }
  echo
  echo "=== rollback OK: board on stock $EXPECT with restored config ==="
} 2>&1 | tee "$LOG"

rc=${PIPESTATUS[0]}
echo "log: $LOG"
exit "$rc"
