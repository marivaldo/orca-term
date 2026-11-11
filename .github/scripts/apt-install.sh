#!/usr/bin/env bash
# Installs Ubuntu packages on a GitHub Linux runner: `apt-install.sh <apt-get install args>`.
#
# The runners point apt at Azure's copy of the archive first (`azure.archive.ubuntu.com`, through
# `/etc/apt/apt-mirrors.txt`). When that name stops resolving, every job that installs a package
# fails before running anything (seen for hours on 2026-11-11). So apt is pointed at Ubuntu's own
# archive instead, still signed by Ubuntu's keys, and retries a download that fails.
set -euo pipefail

mirrors=/etc/apt/apt-mirrors.txt
if [ -f "$mirrors" ]; then
  printf '%s\n' \
    'http://archive.ubuntu.com/ubuntu/ priority:1' \
    'http://security.ubuntu.com/ubuntu/ priority:2' |
    sudo tee "$mirrors" >/dev/null
fi

retries=(-o Acquire::Retries=5)
sudo apt-get "${retries[@]}" update
sudo apt-get "${retries[@]}" install --yes "$@"
