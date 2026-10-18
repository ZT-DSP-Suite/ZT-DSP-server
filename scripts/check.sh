#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 ZT-DSP Suite contributors
#
# Repository guard. Each check reports its own result; a failed check does not
# stop the remaining ones from reporting. Exit status is non-zero if any failed.

set -u

fails=0
note() { printf '  ok   %s\n' "$*"; }
bad() { printf '  FAIL %s\n' "$*"; fails=$((fails + 1)); }

if ! git rev-parse --git-dir >/dev/null 2>&1; then
  printf '  skip git checks (not a git work tree)\n'
  files() { find . -type f -not -path './.git/*' -not -path './target/*' "$@"; }
else
  # tracked + untracked-not-ignored, so the guard works before and after git add
  files() { git ls-files -co --exclude-standard -- "$@" 2>/dev/null; }
fi

echo '== 1. licence headers =='
hdr_fail=0
while IFS= read -r f; do
  [ -n "$f" ] || continue
  case "$f" in
    *.rs|*.sh|*.py)
      if ! head -5 "$f" | grep -q 'SPDX-License-Identifier'; then
        bad "$f: missing SPDX-License-Identifier"
        hdr_fail=1
      fi
      ;;
  esac
done < <(files '*.rs' '*.sh' '*.py')
[ $hdr_fail -eq 0 ] && note 'rs/sh/py carry SPDX headers'

echo '== 2. no identity material in logs =='
if files 'src/*.rs' | xargs -r grep -nE '(info|warn|error|debug|trace)!\([^)]*(token_hash|session_id|\.identity)' 2>/dev/null; then
  bad 'a log macro formats identity material'
else
  note 'no log macro formats token hashes or session ids'
fi

echo '== 3. bilingual mirrors =='
mir_fail=0
for f in README.md cn/README.md en/README.md; do
  if [ ! -s "$f" ]; then
    bad "$f missing or empty"
    mir_fail=1
  fi
done
[ $mir_fail -eq 0 ] && note 'mirrors present and non-empty'

echo '== 4. no placeholder wording =='
if files 'README.md' 'cn/**' 'en/**' | xargs -r grep -niE 'placeholder repository|尚未开始实现|no implementation yet' 2>/dev/null; then
  bad 'placeholder wording still present in a README'
else
  note 'no placeholder wording'
fi

echo '== 5. no stale doc references =='
# docs must not cite source symbols that do not exist
if files 'docs/*.md' | xargs -r grep -nE 'parse_phase4_header|TokenHash::hex_prefix|LOBBY_TIMEOUT|Phase4Header' 2>/dev/null; then
  bad 'docs reference symbols removed from the source'
else
  note 'no stale symbol references in docs'
fi

echo
if [ $fails -ne 0 ]; then
  printf 'check.sh: FAILED (%d)\n' "$fails"
  exit 1
fi
printf 'check.sh: ok\n'
