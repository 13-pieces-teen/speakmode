# README 素材说明

README 介绍当前桌面源码 0.2.37。展示素材分为品牌插画、流程示意和当前前端截图，所有仓库引用均位于 `docs/assets/readme/`。

## 素材清单

| 文件 | 类型与来源 |
| --- | --- |
| `hero.webp` | 本轮使用内置 imagegen 生成的品牌插画，无应用界面或效果指标；由 PNG 无损转换为 WebP。 |
| `status.svg` | 项目原生矢量标签，标明源码版本、平台、自备 Key 与技术栈。 |
| `workflow.gif` | 已制作的 HyperFrames 流程示意，12 秒、960 × 540、18 fps；展示自定义快捷键，用于说明开始录音、整理和输入文本框的顺序。 |
| `home.png` | 当前生产前端首页，显示固定邮件示例。 |
| `scenes.png` | 同一首页展开五种内置场合菜单。 |
| `history.png` | 当前历史详情，展开识别稿与成稿对照。 |
| `dictionary.png` | 当前个人词典，使用九个预置示例词。 |
| `personalization.png` | 当前个性化页面，使用示例身份与表达习惯。 |
| `providers.png` | 当前服务配置，展开文字整理供应商菜单。 |

流程 GIF 解释交互顺序，不逐像素复刻当前界面。六张截图使用生产 React 构建，固定数据经测试专用 bridge 注入；其中的时长、字数、已保存状态、文本和画像均为演示数据，不是服务实测或用户记录。截图没有修改产品布局、样式或功能。

## 截图复现

在仓库根目录运行：

```powershell
npm --prefix app run build
python scripts/capture-readme-assets.py
```

工具复用已有的隔离浏览器夹具，只访问本机回环地址。需要本机已有 Python Playwright 与 Microsoft Edge。它临时启动并关闭生产构建预览，截图写入素材目录，采集收据写入 `.validation/readme-assets/` 的独立子目录。

## 封面生成

使用 **内置 imagegen**，未调用 CLI/API 回退。保持产品蓝白配色，以声纹转为纸页表达产品用途；标题为 SpeakMode，口号为“说得自然，写得清楚。”。

完整生成提示词：

```text
Use case: ads-marketing
Asset type: a polished wide GitHub README hero banner for the existing Windows voice-input application SpeakMode, aspect ratio about 2.7:1, crisp high-resolution landscape.
Primary request: communicate the transformation from spoken thoughts into clear usable writing, in a calm, premium, editorial product illustration.
Visual identity: very pale blue-white background #F1F6FC, deep navy ink #17304C, one restrained cobalt accent #2B68C9, white paper and soft blue-gray shadows. Keep the product's quiet blue-white aesthetic.
Composition: on the left 45%, exceptionally clean typography and generous margin; on the right 55%, an elegant sculptural sound waveform made from seven rounded cobalt and frosted-blue vertical rods, flowing along a curved fine line into two neat floating white paper sheets with a few navy text-like horizontal strokes. Fine architectural grid details and shallow tactile relief, sophisticated soft studio lighting, no glass UI.
Text (verbatim): large wordmark "SpeakMode"; below it in highly legible Chinese sans-serif "说得自然，写得清楚。"; small quiet descriptor "Windows 语音输入与文字整理". These are the only words.
Layout: headline should read instantly at 900px wide. Right-hand illustration should be a single coherent sculpture, no cards collage, no app screenshot, no diagram arrows. Keep all text at least 80px inside the edges.
Constraints: retain exact spelling SpeakMode, exact Chinese text, no fabricated application interface, no third-party logos, no badges, no statistics or speed claims, no photo of people, no microphone icon, no neon cyan, no purple, no black background, no watermark. The existing brand's five-line waveform may be suggested, but do not invent a complicated new logo.
```

## README 呈现参考

- [Handy README](https://github.com/cjpais/Handy/blob/main/README.md)：一句话定位、具体价值、简短使用步骤与快速开始。
- [Bruno README](https://github.com/usebruno/bruno/blob/main/readme.md)：品牌首屏、少量信息标签、产品截图与导航。

参考的是信息顺序与阅读体验，没有复制其他项目的图片、品牌图形或产品效果声明。SpeakMode 的功能文案依据本仓库当前源码，不复用竞品的离线、跨平台或许可承诺。
