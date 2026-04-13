#!/bin/sh
# Replace __MTX_NAME__ placeholder with the actual instance name
if [ -n "$MTX_NAME" ] && [ -f /mediamtx.yml ]; then
    sed "s/__MTX_NAME__/${MTX_NAME}/g" /mediamtx.yml > /tmp/mediamtx.yml
    CONFIG="/tmp/mediamtx.yml"
else
    CONFIG="/mediamtx.yml"
fi

# Trap SIGTERM — drain before shutdown
cleanup() {
    echo "SIGTERM received, draining $MTX_NAME..."
    curl -s -X POST "http://api:8080/internal/mtx/drain?mtx=$MTX_NAME" || true
    sleep 5
    kill -TERM "$MTX_PID" 2>/dev/null
    wait "$MTX_PID" 2>/dev/null
}
trap cleanup SIGTERM

# Start MediaMTX in background
/usr/local/bin/mediamtx "$CONFIG" &
MTX_PID=$!
wait "$MTX_PID"
