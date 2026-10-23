#!/bin/sh
IFS= read -r mode < "$0.mode" || exit 65
LOG="$0.log"
printf '%s\n' "$1" >> "$LOG"
hang_forever() {
  while :; do :; done
}
case "$1" in
  info)
    if [ "$mode" = "hung-info" ]; then hang_forever; fi
    exit 0
    ;;
  create)
    case "$mode" in
      hung-create) hang_forever ;;
      create-empty) exit 0 ;;
      create-bad) printf 'not-a-container-id\n'; exit 0 ;;
      create-delayed-bad) sleep 0.2; printf 'not-a-container-id\n'; exit 0 ;;
      *) printf '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n'; exit 0 ;;
    esac
    ;;
  start)
    case "$mode" in
      normal|cleanup-fail) printf 'completed\n'; exit 0 ;;
      nonzero-cleanup-fail) printf 'command failed\n' >&2; exit 17 ;;
      large-normal) yes x | head -c 262144; exit 0 ;;
      large-timeout|large-cancel)
        yes x | head -c 65536
        printf 'large-ready\n' >> "$LOG"
        exec yes x
        ;;
      timeout|cancel|timeout-cleanup-fail|cancel-cleanup-fail|abort|blocked-stdin) exec sleep 10 ;;
    esac
    ;;
  rm)
    case "$mode" in
      hung-rm) hang_forever ;;
      global-first-fail)
        if [ "$3" = "first-container" ]; then
          printf 'forced first cleanup failure\n' >&2
          exit 24
        fi
        exit 0
        ;;
      cleanup-fail|*-cleanup-fail)
        printf 'forced cleanup failure\n' >&2
        exit 23
        ;;
      *) exit 0 ;;
    esac
    ;;
  ps)
    if [ "$mode" = "global-first-fail" ]; then
      printf 'first-container\nsecond-container\n'
    elif [ "$mode" = "global-truncated" ]; then
      printf 'first-container\n'
      yes a | tr -d '\n' | head -c 70000
    fi
    exit 0
    ;;
esac
exit 64
