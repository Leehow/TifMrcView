#!/usr/bin/env bash
# Installs apt packages on a CI runner without hanging for an hour when the
# runner's own package mirror is unreachable: short timeouts, and after a
# failed try the main Ubuntu archive instead of the mirror.
set -u
opts=(-o Acquire::Retries=2 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30 -o Acquire::ForceIPv4=true)
for try in 1 2 3; do
  if sudo apt-get "${opts[@]}" update && sudo apt-get "${opts[@]}" install -y "$@"; then
    exit 0
  fi
  echo "apt failed (try $try); switching to archive.ubuntu.com"
  if [ -f /etc/apt/apt-mirrors.txt ]; then
    printf 'http://archive.ubuntu.com/ubuntu/\tpriority:1\n' | sudo tee /etc/apt/apt-mirrors.txt >/dev/null
  fi
  sudo sed -i 's#azure\.archive\.ubuntu\.com#archive.ubuntu.com#g' /etc/apt/sources.list /etc/apt/sources.list.d/* 2>/dev/null || true
  sleep 10
done
exit 1
