# 🔄 POE to OpenAI API

[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Docker Version](https://img.shields.io/docker/v/jeromeleong/poe2openai?sort=semver)](https://hub.docker.com/r/jeromeleong/poe2openai)
[![Docker Size](https://img.shields.io/docker/image-size/jeromeleong/poe2openai/latest
)](https://hub.docker.com/r/jeromeleong/poe2openai)
[![Docker Pulls](https://img.shields.io/docker/pulls/jeromeleong/poe2openai)](https://hub.docker.com/r/jeromeleong/poe2openai)

Poe2OpenAI 是一個將 POE API 轉換為 OpenAI API 格式的代理服務。讓 Poe 訂閱者能夠通過 OpenAI API 格式使用Poe 的各種AI模型

## 📑 目錄
- [主要特點](#-主要特點)
- [安裝指南](#-安裝指南)
- [快速開始](#-快速開始)
- [API 文檔](#-api-文檔)
- [配置說明](#️-配置說明)
- [常見問題](#-常見問題)
- [貢獻指南](#-貢獻指南)
- [授權協議](#-授權協議)

## ✨ 主要特點
- 🔄 支援 OpenAI API 格式（/models 和 /chat/completions）
- 💬 支援串流和非串流模式
- 🔧 支援工具調用 (Tool Calls)
- 📊 Web 管理介面用於配置模型（模型映射 和 編輯/models 顯示的模型）
- 🚀 Rust 實現
- 🌐 對 POE API 的 Event 進行完整處理
- 🐳 Docker 佈置支持
- 🤖 Cline/Roo Code 支持

## 🔧 安裝指南

### 使用 Docker（簡單部署）

```bash
# 拉取映像
docker pull jeromeleong/poe2openai:latest

# 運行容器
docker run --name poe2openai -d \
  -p 8080:8080 \
  -e ADMIN_USERNAME=admin \
  -e ADMIN_PASSWORD=123456 \
  jeromeleong/poe2openai:latest
```

#### 數據持久化（可選）

```bash
# 創建本地數據目錄
mkdir -p /path/to/data

# 運行容器並掛載數據目錄
docker run --name poe2openai -d \
  -p 8080:8080 \
  -v /path/to/data:/data \
  -e ADMIN_USERNAME=admin \
  -e ADMIN_PASSWORD=123456 \
  jeromeleong/poe2openai:latest
```

### 使用 Docker Compose

具體內容可根據自己個人需求來進行修改
```yaml
version: '3.8'
services:
  poe2openai:
    image: jeromeleong/poe2openai:latest
    ports:
      - "8080:8080"
    environment:
      - PORT=8080
      - LOG_LEVEL=info
      - ADMIN_USERNAME=admin
      - ADMIN_PASSWORD=123456
      - MAX_REQUEST_SIZE=1073741824
      - CONFIG_DIR=/data
    volumes:
      - /path/to/data:/data
```

### 從源碼編譯

```bash
# 克隆專案
git clone https://github.com/jeromeleong/poe2openai
cd poe2openai

# 編譯
cargo build --release

# 運行
./target/release/poe2openai
```

## 🚀 快速開始

1. 使用 Docker 啟動服務：
```bash
docker run -d -p 8080:8080 jeromeleong/poe2openai:latest
```

2. 服務器默認在 `http://localhost:8080` 啟動

3. 使用方式示例：
```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer your-poe-token" \
  -d '{
    "model": "gpt-4o-mini",
    "messages": [{"role": "user", "content": "你好"}],
    "stream": true
  }'
```

4. 可以在`http://localhost:8080\admin`管理模型

## 📖 API 文檔

### 支援的 Openai API端點

- `GET /v1/models` - 獲取可用模型列表
- `POST /v1/chat/completions` - 與 POE 模型聊天
- `GET /models` - 獲取可用模型列表（相容端點）
- `POST /chat/completions` - 與 POE 模型聊天（相容端點）

### 請求格式
```json
{
  "model": "string",
  "messages": [
    {
      "role": "user",
      "content": "string"
    }
  ],
  "temperature": 0.7,
  "stream": false
}
```

### 響應格式

```json
{
  "id": "chatcmpl-xxx",
  "object": "chat.completion",
  "created": 1677858242,
  "model": "gpt-4o-mini",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "回應內容"
      },
      "finish_reason": "stop"
    }
  ]
}
```

## ⚙️ 配置說明

服務器配置通過環境變量進行：

- `PORT` - 服務器端口（默認：`8080`）
- `HOST` - 服務器主機（默認：`0.0.0.0`）
- `ADMIN_USERNAME` - 管理介面用戶名	默認：`admin`）
- `ADMIN_PASSWORD` - 管理介面密碼	默認：`123456`）
- `MAX_REQUEST_SIZE` - 最大請求大小（默認：`1073741824`）
- `LOG_LEVEL` - 日誌級別（默認：`info`）
- `CONFIG_DIR` - 配置文件目錄路徑（docker 環境中默認為：`/data`，本機環境中默認為：`./`）

## ❓ 常見問題

### Q: Poe API Token如何獲取？
A: 首先要訂閱Poe，才能從[Poe API Key](https://poe.com/api_key)網頁中取得

### Q: 為什麼會收到認證錯誤？
A: 確保在請求頭中正確設置了 `Authorization: Bearer your-poe-token`

### Q: 支援哪些模型？
A: 支援所有 POE 平台上可用的模型，可通過 `/v1/models` 端點查詢

### Q: 如何修改服務器端口？
A: 可以通過設置環境變量 `PORT` 來修改，例如：
```bash
docker run -d -e PORT=3000 -p 3000:3000 jeromeleong/poe2openai:latest
```

## 🤝 貢獻指南

歡迎所有形式的貢獻！

## 📄 授權協議

本專案使用 [MIT 授權協議](LICENSE)。

## 🌟 Star 歷史

[![Star History Chart](https://api.star-history.com/svg?repos=jeromeleong/poe2openai&type=Date)](https://star-history.com/#jeromeleong/poe2openai&Date)