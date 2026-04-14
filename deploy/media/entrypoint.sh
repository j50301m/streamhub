#!/bin/sh
# Replace placeholders in mediamtx.yml:
# - __MTX_NAME__ = instance name (for webhooks)
# - __WEBRTC_UDP_PORT__ = WebRTC UDP port (must match host port for ICE to work
#   across docker port mapping; each instance has a unique port)
WEBRTC_PORT="${WEBRTC_UDP_PORT:-8189}"
if [ -f /mediamtx.yml ]; then
    sed -e "s/__MTX_NAME__/${MTX_NAME:-mtx-unknown}/g" \
        -e "s/__WEBRTC_UDP_PORT__/${WEBRTC_PORT}/g" \
        /mediamtx.yml > /tmp/mediamtx.yml
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
