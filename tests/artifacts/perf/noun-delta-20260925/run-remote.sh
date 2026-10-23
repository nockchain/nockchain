#!/usr/bin/env bash
set -Eeuo pipefail
run=/opt/nockcloud/backups/noun-delta-bench-20260925
input=/opt/nockcloud/backups/pma-storage-efficiency-20260925T1635Z/data/pma
label=$1
shift
[[ "$label" =~ ^[a-z0-9-]+$ ]]
[[ $(systemctl show nockchain-pma-candidate-20260925.service --property=MainPID --value) == 0 ]]
systemctl is-active --quiet nockcloud-pool-leader.service
systemd-run --unit="nockchain-noun-$label" \
 -p Type=exec -p Restart=no -p RuntimeMaxSec=900 -p TimeoutStopSec=10 \
 -p PrivateNetwork=yes -p ProtectSystem=strict -p ProtectHome=yes \
 -p PrivateTmp=yes -p PrivateDevices=yes -p NoNewPrivileges=yes \
 -p CapabilityBoundingSet= -p RestrictNamespaces=yes \
 -p ProtectKernelTunables=yes -p ProtectKernelModules=yes -p ProtectControlGroups=yes \
 -p "ReadWritePaths=$run" -p "BindReadOnlyPaths=$input:/inputs" \
 -p 'InaccessiblePaths=/opt/nockcloud/pool-leader /etc/nockcloud' \
 -p CPUQuota=800% -p MemoryHigh=144G -p MemoryMax=176G -p MemorySwapMax=0 \
 -p IOWeight=20 -p "WorkingDirectory=$run" \
 -p "StandardOutput=append:$run/$label.jsonl" \
 -p "StandardError=append:$run/$label.stderr" \
 /usr/bin/time -v -o "$run/$label.time" "$run/noun-delta-bench" "$@"
