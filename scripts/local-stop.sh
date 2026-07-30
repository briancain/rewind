#!/bin/bash
# scripts/local-stop.sh — Stops all local dev services
set -e

echo "Stopping backend services..."
pkill -f "target/debug/identity" 2>/dev/null || true
pkill -f "target/debug/video-catalog" 2>/dev/null || true
pkill -f "target/debug/upload" 2>/dev/null || true
pkill -f "target/debug/streaming" 2>/dev/null || true
pkill -f "target/debug/social" 2>/dev/null || true
pkill -f "target/debug/search" 2>/dev/null || true
pkill -f "target/debug/transcode" 2>/dev/null || true
echo "  ✓"

echo "Stopping containers..."
cd "$(dirname "$0")/.."
finch compose down 2>/dev/null || docker compose down 2>/dev/null || true
echo "  ✓"

echo "Stopping frontend..."
pkill -f "next dev" 2>/dev/null || true
echo "  ✓"

rm -rf /tmp/rewind-logs
echo "Done."
