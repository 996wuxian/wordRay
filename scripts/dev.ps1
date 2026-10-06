# 启动 WordRay 开发模式（推荐入口）。
#
# 存在的意义有两个：
#   1. 裸跑 `tauri`（不带子命令）只会打印帮助并 exit 2，容易踩；
#   2. DeepSeek Key 必须与 `tauri dev` 在**同一个进程环境**里，
#      而 IDE 任务面板里往往没有你刚在别的终端设的 Key——所以这里直接问你。
#
# 用法：
#   .\scripts\dev.ps1
#   .\scripts\dev.ps1 -Force      # 没有 Key 也直接启动

param(
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

# 切到仓库根目录（本脚本在 scripts/ 下）
Set-Location (Split-Path -Parent $PSScriptRoot)

if (-not $env:DEEPSEEK_API_KEY) {
    Write-Host ''
    Write-Host '未检测到环境变量 DEEPSEEK_API_KEY。' -ForegroundColor Yellow
    Write-Host 'Key 只放在进程环境变量里，不写入任何文件。' -ForegroundColor DarkGray
    Write-Host ''

    $entered = Read-Host '请粘贴 DeepSeek API Key（直接回车 = 跳过）'

    if ($entered) {
        $env:DEEPSEEK_API_KEY = $entered.Trim()
        Write-Host '已设置（只存在于本次进程环境，不落盘）。' -ForegroundColor Green
    }
    elseif (-not $Force) {
        $answer = Read-Host '没有 Key 也可以启动，但按热键会提示缺少 Key。仍要启动吗？(y/N)'
        if ($answer -ne 'y') {
            Write-Host '已取消。' -ForegroundColor DarkGray
            return
        }
    }
}

if (-not $env:DEEPSEEK_BASE_URL) { $env:DEEPSEEK_BASE_URL = 'https://api.deepseek.com/v1' }
if (-not $env:DEEPSEEK_MODEL) { $env:DEEPSEEK_MODEL = 'deepseek-chat' }

Write-Host ''
Write-Host '模型配置：' -ForegroundColor DarkGray
Write-Host ("  base_url = {0}" -f $env:DEEPSEEK_BASE_URL) -ForegroundColor DarkGray
Write-Host ("  model    = {0}" -f $env:DEEPSEEK_MODEL) -ForegroundColor DarkGray
Write-Host ("  api_key  = {0}" -f $(if ($env:DEEPSEEK_API_KEY) { '已设置（不显示）' } else { '未设置' })) -ForegroundColor DarkGray
Write-Host ''
Write-Host '启动后：选中中文 → 按控制台提示的热键 → 面板显示译文。' -ForegroundColor DarkGray
Write-Host '退出：在本控制台按 Ctrl+C（面板上的 ✕ 只是隐藏窗口）。' -ForegroundColor DarkGray
Write-Host ''

pnpm tauri dev
