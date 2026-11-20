#!/bin/sh
# Name: Kindle Dashboard
# Author: Dashboard
# Icon: /mnt/us/kindle-dashboard/library-cover.png
# DontUseFBInk

root=/mnt/us/kindle-dashboard
[ -r "$root/supervisor.sh" ] || exit 1
exec /bin/sh "$root/supervisor.sh" launch panel
