# SPEC-006 VOD 回放

狀態：done

## 目標

直播結束後自動將錄影 MP4 轉為 HLS 串流格式，觀眾可回放觀看。

## 影響範圍

新增 / 修改：
- `deploy/services/Dockerfile.api` — 加 ffmpeg
- `crates/transcoder/src/` — ffmpeg 呼叫封裝（MP4 → HLS）
- `crates/hook/src/publish.rs` — unpublish 時觸發轉檔
- `crates/entity/src/stream.rs` — 加 hls_url 欄位
- `crates/api/src/routes/streams.rs` — stream response 加 hls_url、recordings 改公開
- `deploy/services/nginx.conf` — 加 /vod/ location serve HLS
- `deploy/services/docker-compose.yml` — web service 也 mount recordings volume
- `web/viewer/index.html` — VOD 回放 UI

## Todo list

- [x] SPEC-006-01 Dockerfile.api — 加 ffmpeg 到 runtime image
- [x] SPEC-006-02 transcoder crate — ffmpeg 封裝：輸入 MP4 路徑，輸出 HLS（m3u8 + ts）到指定目錄
- [x] SPEC-006-03 entity — streams 加 hls_url (Option<String>, nullable)
- [x] SPEC-006-04 hook — unpublish 時 spawn tokio task 執行轉檔：vod_status Processing → Ready/Failed，成功時寫 hls_url
- [x] SPEC-006-05 nginx — 加 /vod/ location，serve /recordings/ 目錄的 HLS 檔案，設定 CORS
- [x] SPEC-006-06 docker-compose — web service mount recordings volume（讓 nginx 能 serve HLS）
- [x] SPEC-006-07 API — stream response 加 hls_url、GET /v1/streams/:id/recordings 改為公開
- [x] SPEC-006-08 viewer — Ended + Ready 的 stream 顯示「回放」按鈕，用 hls.js 播放 VOD

## 驗收流程

```bash
cd deploy
docker compose up --build -d
```

1. broadcaster 登入 → 建流 → 推流 30 秒 → 停止
2. 等待轉檔完成（API log 顯示 ffmpeg 完成）
3. viewer 頁面 → stream card 顯示「回放」→ 點擊播放 HLS
4. 確認可 seek、可暫停、播放流暢

## 備註

- ffmpeg 轉檔參數：`ffmpeg -i input.mp4 -c copy -hls_time 6 -hls_list_size 0 -hls_segment_filename 'seg_%03d.ts' index.m3u8`
- `-c copy` 不重新編碼，只是重新封裝，速度很快
- HLS 輸出到 `/recordings/{stream_key}/hls/` 目錄
- hls_url 格式：`/vod/{stream_key}/hls/index.m3u8`
- 一個 stream 可能有多段 MP4 錄影，轉檔時合併或分別轉（先做分別轉，簡單）
- 轉檔是非同步的（tokio::spawn），不阻塞 webhook 回應
