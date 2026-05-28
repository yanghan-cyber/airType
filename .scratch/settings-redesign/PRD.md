# 设置页面重构

## Problem Statement

当前设置页面的 Tab 分组（通用/热词/高级/AI）不符合用户使用心智：
- 「热词」单独成 Tab，但它是 ASR 语音识别的一部分
- 「高级」和「AI」分不清边界——ASR 后端配置和 LLM 配置分在两个 Tab
- 「通用」Tab 塞了太多不相关的东西（启用开关、快捷键、语言、处理模式、胶囊位置）
- 保存逻辑不一致：有 4 条独立的保存路径（`saveConfig`/`saveLlmConfig`/`saveCapsuleDefaults`/`saveProcessingMode`），各自 load→modify→save 整个 config，快速操作时可能互相覆盖
- 「第二录音热键」命名不直观

## Solution

按用户使用流程重新组织为 3 个 Tab，统一保存行为：

| Tab | 包含内容 | 用户心理 |
|-----|---------|---------|
| 🎙️ 语音输入 | 启用开关、快捷键（主+AI录音键）、语言、ASR 后端/模型/热词、后端状态 | "我要让语音识别好用" |
| 🤖 AI 处理 | 默认处理模式、LLM 连接配置、提示词管理 | "我要让 AI 处理好用" |
| 🎨 界面 | 胶囊位置、偏移量、自定义位置 | "我要调悬浮窗" |

保存行为：
- 开关/下拉框：**即时保存**，修改后立即生效
- 文本输入框：**失焦保存**，输入框失去焦点时保存
- API 配置（后端地址、API Key）：失焦保存 + **测试连接按钮**验证
- 后端命令接口不变：前端仍调用 `save_config`（语音输入 Tab）和 `save_llm_config`（AI 处理 Tab），只是统一了触发时机

## User Stories

1. As a 用户，我希望打开设置页面就能看到所有语音识别相关的配置，这样我不用在多个 Tab 之间跳转
2. As a 用户，我希望切换"启用语音输入"开关后立即生效，不需要额外点保存按钮
3. As a 用户，我希望修改录音快捷键后立即生效，这样我可以马上用新快捷键测试
4. As a 用户，我希望 AI 录音键（原第二热键）有清晰的命名，让我知道它的用途
5. As a 用户，我希望热词管理在语音输入 Tab 内，因为热词是 ASR 的一部分
6. As a 用户，我希望 ASR 后端地址和 API Key 改完后能测试连接，确认配置正确
7. As a 用户，我希望 LLM 配置和提示词管理在同一个 Tab，因为它们共同决定 AI 处理效果
8. As a 用户，我希望修改 LLM Base URL 后能测试连接，确认配置正确
9. As a 用户，我希望默认处理模式在 AI 处理 Tab 里选择，和其他 AI 配置放一起
10. As a 用户，我希望胶囊位置设置独立成 Tab，不干扰核心功能配置
11. As a 用户，我希望输入框在失去焦点时自动保存，不需要找保存按钮
12. As a 用户，我希望所有设置修改后底部状态栏显示"已保存"反馈
13. As a 用户，我希望提示词列表可以拖拽排序，前 3 项自动显示在 AI 录音键弹窗中
14. As a 用户，我希望添加/编辑/删除提示词的操作在同一 Tab 内完成

## Implementation Decisions

### 模块划分

**前端（`ui/settings.html`）**
- 重写 Tab 结构：4 Tab → 3 Tab
- 统一保存时机：开关/下拉框用 `onchange` 即时保存，文本框用 `onblur` 失焦保存
- "第二录音热键" 改名为 "AI 录音键"
- 保存函数保持现有拆分：语音输入 Tab 调 `saveConfig()`，AI 处理 Tab 调 `saveLlmConfig()`，界面 Tab 调 `saveCapsuleDefaults()`

**后端（`src-tauri/src/commands.rs`）**
- 保持现有命令接口不变（`save_config`、`save_llm_config` 等）
- 后端命令已经是幂等的（load→modify→save），前端统一调用时机即可
- 不需要新增后端命令

### 保存行为规范

| 控件类型 | 保存时机 | 调用函数 |
|---------|---------|---------|
| 开关（toggle） | 点击即时 | `saveConfig()` |
| 下拉框（select） | 选择即时 | 对应 Tab 的保存函数 |
| 文本输入框（语音 Tab） | 失焦保存 | `saveConfig()` |
| 文本输入框（AI Tab） | 失焦保存 | `saveLlmConfig()` |
| 文本输入框（界面 Tab） | 失焦保存 | `saveCapsuleDefaults()` |
| 快捷键录制 | 录制完成即时 | `saveConfig()` 或 `saveSecondaryHotkey()` |
| 热词添加/删除 | 操作即时 | `saveConfig()` |
| Extra Params（textarea） | 失焦保存 | `saveLlmConfig()` |

### Tab 分组详情

**🎙️ 语音输入 Tab**
- 启用语音输入（toggle）
- 录音快捷键（hotkey recorder）
- AI 录音键（hotkey recorder，原"第二录音热键"）
- 语言（text input）
- 分组：ASR 后端
  - 后端地址（text input + 刷新按钮）
  - API Key（password input + 显示/隐藏按钮）
  - ASR 模型（text input + 刷新按钮 + 下拉选择）
- 热词（tag list + 添加输入框）
- 分组：后端状态
  - 连接状态指示

**🤖 AI 处理 Tab**
- 默认处理模式（select）
- 分组：LLM 连接
  - Base URL（text input）
  - API Key（password input + 显示/隐藏按钮）
  - 模型（text input + 刷新按钮 + 下拉选择）
  - Temperature + Max Tokens（并排 number input）
  - 测试连接按钮
- 分组：请求参数
  - Extra Params（textarea）
- 分组：提示词管理
  - 提示词列表（可排序）
  - 添加新模式按钮

**🎨 界面 Tab**
- 默认位置（select：底部居中/顶部居中）
- 距边缘偏移（number input + px 单位）
- 自定义位置（调整位置按钮 + 恢复默认按钮）

## Testing Decisions

- 验证 Tab 切换正确显示/隐藏对应面板
- 验证开关即时保存（修改后刷新页面，值保持）
- 验证输入框失焦保存（修改后点击别处，值保持）
- 验证 AI 录音键名称显示正确
- 验证热词在语音输入 Tab 内正常添加/删除
- 验证 LLM 测试连接按钮功能正常
- 验证提示词列表排序、编辑、删除功能

## Out of Scope

- 后端命令接口重构（保持现有接口不变）
- 新增设置项（如主题色、透明度等）
- 配置文件格式变更
- 移动端适配

## Further Notes

- 设计系统沿用现有的 Spotify 风格（DESIGN.md）
- mockup 文件在 `.superpowers/brainstorm/` 目录下可参考
