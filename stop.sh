#!/bin/bash

INSTALL_ROOT="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PID_FILE="$INSTALL_ROOT/reth.pid"
PID="$( cat $PID_FILE )"

if [ -n "$PID" ]; then
  echo "Killing pid " $PID
  kill $PID

  for i in $(seq 1 20); do
	IS_RUNNING=`ps $PID | wc -l`

	if [ $IS_RUNNING = "1" ]; then
		echo "$INSTALL_ROOT node has been shutdown"
		break;
	fi

	echo "Waiting..."

	sleep 2
  done

  if [ $IS_RUNNING = "2" ]; then
	echo "ERROR: Unable to shutdown $INSTALL_ROOT node successfully, check log"
  fi

else
  echo "No pid found at $PID_FILE"
fi