#!/bin/sh
# Replace __MTX_NAME__ placeholder with the actual instance name from environment
if [ -n "$MTX_NAME" ] && [ -f /mediamtx.yml ]; then
    sed "s/__MTX_NAME__/${MTX_NAME}/g" /mediamtx.yml > /tmp/mediamtx.yml
    exec /usr/local/bin/mediamtx /tmp/mediamtx.yml
else
    exec /usr/local/bin/mediamtx /mediamtx.yml
fi
