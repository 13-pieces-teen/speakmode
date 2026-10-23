# 第三方来源与许可材料

更新：2026-10-11。本目录随 SpeakMode 0.1.4 源码整理上传，0.2.12 补入 Chromium 兼容缓存哈希的许可材料，保留已有第三方原文、修改说明与安装器资源。它记录已找到的材料和未解决的证据缺口，不表示已完成发行件法律审查、安装验收或供应链签名验证。第三方文件适用各自许可证；本次整理没有为 SpeakMode 的主代码新增 MIT 或其他开源授权。

## 直接纳入的代码

| 内容 | 来源与本地材料 | 修改边界 |
| --- | --- | --- |
| 全局键盘库 handy-keys 0.3.4 | [固定上游提交 e7d1b8e6](https://github.com/handy-computer/handy-keys/tree/e7d1b8e6de8258f21ee1d60608d16f8d195157bf)；[vendored MIT LICENSE](../../app/src-tauri/src/platform/vendor/handy-keys/LICENSE)，版权为 2026 handy-computer | Cargo 使用本地 patch；该目录不是上游的逐字节副本。[平台说明](../../app/src-tauri/src/platform/README.md)与[本地修改记录](LOCAL-SOURCE-MODIFICATIONS.md)记录 hook 就绪、事件来源、生命周期及线程释放等改动。 |
| 音频重采样器 | [Handy 固定提交 73ab851c](https://github.com/cjpais/Handy/tree/73ab851c2b6242283759a4c101b60f0ece132f08)；[resampler.rs](../../app/src-tauri/src/platform/resampler.rs)文件头保留来源；[HANDY-LICENSE.txt](../../app/src-tauri/src/platform/HANDY-LICENSE.txt)保留 MIT 与 2025 CJ Pais 版权 | 从 Handy 的音频模块提取，随后修改错误与延迟处理；它与 handy-keys 的许可材料分别保留。 |
| NSIS 安装模板 | [Tauri 固定提交 30da1fd6](https://github.com/tauri-apps/tauri/tree/30da1fd6e17de6107ecc850c95dfb16b5729f2dd)，对应 tauri-cli-v2.12.1；[本地模板](../../app/src-tauri/windows/installer-template.nsi)；原始 [MIT](supplements/tauri-cli-2.12.1-NSIS-template-LICENSE-MIT)、[Apache-2.0](supplements/tauri-cli-2.12.1-NSIS-template-LICENSE-APACHE-2.0)与[SPDX](supplements/tauri-cli-2.12.1-NSIS-template-LICENSE.spdx) | 上游声明 Apache-2.0 OR MIT。本地修改更新路径与数据保留行为，另有[本地 hooks](../../app/src-tauri/windows/installer-hooks.nsh)。详见[修改记录](LOCAL-SOURCE-MODIFICATIONS.md)。模板许可不自动覆盖独立 NSIS compiler/plugins、微软运行时或所有安装器资源。 |

本次清洁源码副本没有纳入完整上游 checkout 或早期 Whisperi 提取评估包。技术方案中对竞品、协议或代码的参考，不应被当作该产品源码已整体复制的证明。

0.2.12 的 [Typeless 缓存读取模块](../../app/src-tauri/src/platform/typeless_dictionary.rs)将 Chromium 使用的 Paul Hsieh SuperFastHash 转写为 Rust，用于兼容缓存的结构校验值与 key hash；不用于密码或安全认证。保留 [BSD 许可原文](supplements/chromium-superfasthash-LICENSE.txt)，相同文件也随安装器分发至 `third-party/supplements/`。Brotli 9.0.0 已存在于既有锁文件及许可清单，本轮改为直接使用，未升级版本。

## 字体与图标

- [speakmode-sans.woff2](../../app/public/fonts/speakmode-sans.woff2)内部字体名称为 **Noto Sans SC Thin**，版本为 **2.004-H2**；版权字符串为 Adobe 2014–2021，保留字体名为 `Source`。文件名及 CSS family `SpeakMode Sans` 是应用使用的别名，并不是原创字体的声明。[同目录 OFL.txt](../../app/public/fonts/OFL.txt)保留版权及完整 SIL Open Font License 1.1。此次读取内部元数据并保留许可，但没有核实该二进制的原始下载地址、上游发布摘要或是否经过子集处理，不能声称字节来源已验证。
- [32x32.png](../../app/src-tauri/icons/32x32.png)、[128x128.png](../../app/src-tauri/icons/128x128.png)与[icon.ico](../../app/src-tauri/icons/icon.ico)是当前应用构建资源。此次整理未找到足以确认其原始生成过程或外部来源的记录，不能据此声称全部图标原创或已完成出处核验。
- 字体的独立说明在此补充；既有汇总 notice 中 Rust 工具链附录的 OFL 文本，不应被误认为这份字体的专门来源证明。

## 安装器资源与冻结快照

[tauri.windows.conf.json](../../app/src-tauri/tauri.windows.conf.json)使用 `../../docs/third-party/nsis-resources/` → 安装目录 `third-party/` 的资源映射。因此仓库保留 `app/` 与 `docs/` 的相对层级，以及 [nsis-resources/](nsis-resources/)内原有 14 个文件；0.2.12 追加一份 SuperFastHash 许可，目前共 14 个分发材料和清单本身。源码压缩包中的 `.crate` 文件是对应依赖的原始源码包，不是应用可执行程序。

- [THIRD_PARTY_NOTICES.txt](nsis-resources/THIRD_PARTY_NOTICES.txt)保留已采集的实际许可正文。
- [依赖清单](nsis-resources/THIRD_PARTY_INVENTORY.json)、[CSV](nsis-resources/THIRD_PARTY_DEPENDENCIES.csv)与[第三方 review](nsis-resources/THIRD_PARTY_REVIEW.md)保留当时的依赖范围和证据边界。
- [distribution-manifest.json](nsis-resources/distribution-manifest.json)保留原分发材料哈希，0.2.12 仅追加 SuperFastHash 许可及其哈希。原 `inputs` 仍属于历史冻结快照，其 Cargo/package 输入哈希不对应当前源码；不能将该快照当成当前完整依赖审计证明。实际分发的逐文件校验与这项历史边界分别记录。
- [原始 MPL 源码包清单](nsis-resources/source-archives/inventory.json)及对应五份 `.crate` 被保留。它们是 cssparser、cssparser-macros、dtoa-short、option-ext 和 selectors 的固定版本原始源码材料。若修改相应依赖，原始包不应冒充修改后的对应源码。
- 原 review、inventory 和修改记录中的部分路径指向原开发环境、完整来源检索包或当时工具链。这些文件保留历史事实，并不是当前 Git 仓库中的可执行步骤或可用本地目录。本次清洁导出没有复制所有原始 provenance JSON、缓存、工具链和检索附件，不能声称凭本仓库即可重放全部历史来源验证。

## 尚未解决的许可正文缺口

既有[第三方 review](nsis-resources/THIRD_PARTY_REVIEW.md)记录以下三个版本仍缺少可确认属于该版本的完整许可正文：

| 依赖 | 已有记录的声明 | 尚缺材料 |
| --- | --- | --- |
| realfft 3.5.0 | MIT | 该版本绑定的权威版权与完整许可正文 |
| unicode-casefold 0.2.0 | MIT / Apache-2.0 | 对应发布版本的权威许可正文及来源关联 |
| react-remove-scroll-bar 2.3.8 | MIT | 对应该 npm 版本的完整许可正文 |

这些缺口保持可见，没有用通用许可证、当前分支或同作者其他库的文件代替。已有声明、来源材料及哈希，不等于最终链接组成、字体图标来源、运行时再分发或实际安装行为已经验收。

## macOS 原生补充

Mac 配置复用上述随附材料，并附带 [MACOS_NATIVE_NOTICES.txt](MACOS_NATIVE_NOTICES.txt)
中的 macOS Keychain 后端许可，以及当前的本地键盘库修改记录。冻结清单仍有上文说明的历史边界，
不因新增 Mac 构建变成对当前所有依赖的重新审计。
