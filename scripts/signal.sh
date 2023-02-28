#!/bin/bash
id=`mmcli -L | awk '{len = split($1, arr, "/"); print arr[len]}'`

while true; do
  signal=`mmcli -mK $id | grep modem.generic.signal-quality.value | awk -F ": " '{ print $2 }'`
  echo `date`" "$signal >> /home/lukas/signal.log
  sleep 15
done
