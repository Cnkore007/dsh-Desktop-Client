//! DeepSeek Harness Desktop Client — Tauri application shell.
//!
//! An original desktop client for DeepSeek Harness: manages the official
//! `dsh web` server lifecycle (spawn/attach/watchdog/logs), provides a
//! branded console page, native clipboard bridging for the web UI, and the
//! standard macOS Edit menu (keyboard copy/paste).
//!
//! Policy: the official DeepSeek Harness sources are NEVER modified. The
//! clipboard bridge that the web UI needs is injected at runtime via a
//! page script (WKWebView `navigator.clipboard.writeText` is unreliable),
//! instead of patching the official frontend bundle.

mod menu;
mod server;

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use server::{DshServer, ServerState};
use tauri::{AppHandle, Manager, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_opener::OpenerExt;

pub struct AppState {
    pub server: DshServer,
    /// URL of the local console page, captured at setup so we can navigate
    /// back to it whenever the service stops.
    pub console_url: Mutex<Option<String>>,
    /// Whether the one-time "minimized to tray" notice has fired this run.
    hide_notice_shown: AtomicBool,
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            server: self.server.clone(),
            console_url: Mutex::new(self.console_url.lock().unwrap().clone()),
            hide_notice_shown: AtomicBool::new(self.hide_notice_shown.load(Ordering::Relaxed)),
        }
    }
}

/// 运行时注入脚本（剪贴板桥 + 插件/命令/技能实时中文注释引擎）：
/// 1. 优先 Tauri 原生剪贴板 → `navigator.clipboard` → execCommand 兜底；
/// 2. 动态扫描并汉化命令、权限预设、内置工具、技能与未来第三方插件描述；
/// 3. 严格遵循零侵入官方源码原则，全部在运行时 DOM/Context 层透明适配。
const RUNTIME_INJECTOR_SCRIPT: &str = r#"(()=>{
  // ── 1. 剪贴板桥 ──────────────────────────────────────────
  if (!window.__dshBridgeInstalled) {
    try {
      var native = window.__TAURI__ && window.__TAURI__.clipboard && window.__TAURI__.clipboard.writeText;
      var orig = navigator.clipboard && navigator.clipboard.writeText;
      if (native || orig) {
        var fn = async function (text) {
          try { if (native) { await native(text); return; } } catch (e) {}
          try { if (orig) { await orig(text); return; } } catch (e) {}
          try {
            var ta = document.createElement('textarea');
            ta.value = text; ta.setAttribute('readonly', '');
            ta.style.position = 'fixed'; ta.style.left = '-9999px';
            document.body.appendChild(ta); ta.select();
            document.execCommand('copy'); ta.remove();
          } catch (e) {}
        };
        if (navigator.clipboard) {
          try { Object.defineProperty(navigator.clipboard, 'writeText', { value: fn, configurable: true }); }
          catch (e) { navigator.clipboard.writeText = fn; }
        }
      }
    } catch (e) {}
    window.__dshBridgeInstalled = true;
  }

  // ── 2. 命令、插件与技能实时中文注释引擎 ────────────────────
  if (!window.__dshI18nInstalled) {
    var I18N_DICT = {
      // 官方命令列表
      "Compact older conversation history": "压缩旧对话历史上下文，节省 Token 并提升响应速度",
      "compact older conversation history": "压缩旧对话历史上下文，节省 Token 并提升响应速度",
      "Download this Session log as a ZIP archive": "将会话日志与轨迹快照导出下载为 ZIP 压缩包",
      "download this session log as a zip archive": "将会话日志与轨迹快照导出下载为 ZIP 压缩包",
      "record feedback about this session": "记录并提交关于本会话的反馈信息",
      "set or view the goal for a long-running task": "设定或查看长时间运行任务的整体目标",
      "Switch the permission preset (sandbox mode + approval policy)": "切换权限预设策略（沙箱隔离模式与审批规则）",
      "switch the permission preset (sandbox mode + approval policy)": "切换权限预设策略（沙箱隔离模式与审批规则）",
      "Enter or leave plan mode": "进入或退出规划设计模式（规划先行）",
      "enter or leave plan mode": "进入或退出规划设计模式（规划先行）",
      "Switch model for this session": "选择本会话使用的大语言模型",
      "Select model for this session": "选择本会话使用的大语言模型",
      "Show help and available commands": "显示帮助信息与可用命令列表",
      "Initialize workspace or repository": "初始化工作区工程或仓库索引",

      // 权限模式与审批
      "Full access": "完全访问权限 (Full access)",
      "full access": "完全访问权限 (Full access)",
      "Read only": "只读安全模式 (Read only)",
      "read only": "只读安全模式 (Read only)",
      "Sandbox mode": "沙箱隔离模式",
      "sandbox mode": "沙箱隔离模式",
      "Ask before changes": "修改前需人工确认 (Ask before changes)",
      "ask before changes": "修改前需人工确认 (Ask before changes)",
      "Auto approve": "自动审批执行 (Auto approve)",
      "auto approve": "自动审批执行 (Auto approve)",

      // 核心工具与技能
      "Execute shell commands in workspace": "在工作区终端中执行 Shell 命令",
      "Search files and inspect directory tree": "搜索工程文件并检视目录树",
      "View, create, or edit files": "查看、创建或精准编辑文件内容",
      "Search the web for current information": "在互联网中检索最新实时信息",
      "Fetch the content of a specific HTTP(S) URL": "抓取并解析指定网页正文内容",
      "Delegate a task to a specialized subagent": "派发子任务给具备独立上下文的子智能体协同执行",
      "The COMPLETE task list, replacing any previous list": "规划与更新任务拆解待办清单",
      "Execute custom multi-step workflow scripts": "执行多步骤自动化工作流脚本",
      "Invoke external Model Context Protocol tools": "调用外部 MCP 协议工具与服务"
    };

    function translateText(text) {
      if (!text || typeof text !== 'string') return text;
      var trimmed = text.trim();
      if (!trimmed) return text;
      if (I18N_DICT[trimmed]) return I18N_DICT[trimmed];
      for (var k in I18N_DICT) {
        if (trimmed.toLowerCase() === k.toLowerCase()) return I18N_DICT[k];
      }

      // 智能通用句式翻译（适应未来任意新增插件与技能）
      if (/^download\s+.+\s+as\s+a\s+zip/i.test(trimmed)) {
        return "导出并下载为 ZIP 压缩包 (" + trimmed + ")";
      }
      if (/^switch\s+(the\s+)?permission/i.test(trimmed)) {
        return "切换权限与安全审批策略 (" + trimmed + ")";
      }
      if (/^enter\s+or\s+leave\s+plan/i.test(trimmed)) {
        return "进入或退出规划模式 (" + trimmed + ")";
      }
      if (/^set\s+or\s+view\s+(the\s+)?goal/i.test(trimmed)) {
        return "设定或查看任务目标 (" + trimmed + ")";
      }
      if (/^compact\s+older\s+conversation/i.test(trimmed)) {
        return "压缩旧对话历史上下文 (" + trimmed + ")";
      }
      if (/^search\s+the\s+web/i.test(trimmed)) {
        return "在线网络检索 (" + trimmed + ")";
      }
      if (/^delegate\s+(a\s+)?task\s+to\s+a\s+subagent/i.test(trimmed)) {
        return "派发任务给子智能体 (" + trimmed + ")";
      }
      if (/^execute\s+.+/i.test(trimmed)) {
        return "执行: " + trimmed;
      }
      return text;
    }

    function translateNode(node) {
      if (!node) return;
      if (node.nodeType === Node.TEXT_NODE) {
        var val = node.nodeValue;
        if (val && val.trim().length > 3) {
          var translated = translateText(val);
          if (translated !== val) node.nodeValue = translated;
        }
      } else if (node.nodeType === Node.ELEMENT_NODE) {
        var walker = document.createTreeWalker(node, NodeFilter.SHOW_TEXT, null, false);
        var cur;
        while ((cur = walker.nextNode())) {
          var txt = cur.nodeValue;
          if (txt && txt.trim().length > 3) {
            var res = translateText(txt);
            if (res !== txt) cur.nodeValue = res;
          }
        }
        if (node.hasAttribute && node.hasAttribute('title')) {
          var t = node.getAttribute('title');
          var tr = translateText(t);
          if (tr !== t) node.setAttribute('title', tr);
        }
      }
    }

    // ── 3. 插件清单全量中文注释与卡片美化 ──────────────────────
    var PLUGIN_DICT = {
      // Cordis 微内核与底层
      "include": "外部配置文件与子模块嵌套加载器",
      "timer": "高精度定时器与周期轮询调度器",
      "hmr": "运行时模块热重载与动态更新 (HMR)",
      "group": "插件分组与批量启停管理",
      "loader": "插件动态拓扑解析与生命周期加载器",
      "cordis": "Cordis 响应式微内核服务框架",
      "cosmokit": "Cordis 核心底层工具与上下文库",

      // LLM 大模型与推理
      "llm": "大语言模型统一抽象接口与调用驱动",
      "llm-deepseek": "DeepSeek 官方模型 (V3/R1/Flash) 原生驱动",
      "llm-pi-ai": "Pi.ai 智能推理引擎驱动与流式转换",
      "llm-retry": "大模型超时重试与速率退避控制器",
      "token-meter": "Token 消耗实时计量与上下文配额监控",

      // 会话与记忆管理
      "session": "会话状态管理、多会话并发与生命周期",
      "session-title": "基于上下文的会话标题自动提取与命名",
      "session-title-first-prompt-llm": "根据首条用户 Prompt 的 LLM 智能会话命名",
      "session-title-llm": "基于多轮对话意图理解的 LLM 智能命名",
      "session-checkpoint-policy": "会话检查点创建与状态快照保存策略",
      "session-log-export": "会话历史日志与执行轨迹 ZIP 打包导出",
      "session-persistence": "会话数据存储抽象与序列化适配器",
      "session-persistence-jsonl": "会话流式 JSONL 本地持久化驱动",
      "session-projection": "会话内存投影与视图状态加速",
      "session-projection-cache": "会话多级视图缓存管理器",
      "session-query": "会话历史消息全文检索与查询",
      "session-query-sqlite": "基于 SQLite 的高性能会话历史索引库",
      "session-reference": "跨会话消息引用与上下文关联服务",
      "session-stats": "会话统计分析与执行性能指标聚合",
      "session-telemetry": "会话诊断遥测与运行状态数据收集",
      "session-telemetry-otel": "OpenTelemetry 标准分布式链路追踪",

      // 智能体与计划
      "agent": "AI 智能体核心决策状态机与事件调度",
      "agent-default-model": "智能体默认推理模型绑定与回退路由",
      "agent-instructions": "系统指令与安全准则动态装配注入",
      "agent-loop": "自主多轮迭代、思考推理与工具调用主循环",
      "agent-presets": "智能体角色预设与系统模板库",
      "agent-tool-presentation": "工具调用执行状态可视化交互渲染",
      "plan-mode": "系统规划设计模式（规划先行与步骤拆解）",
      "persona": "智能体人设偏好、交互风格与语气配置",
      "system-prompt": "动态系统提示词构建与上下文装配",

      // 上下文压缩与优化
      "compaction": "长会话上下文智能压缩引擎 (节省 Token)",
      "compaction-basic": "基础历史消息摘要压缩与滑动窗口清理",
      "compaction-tool-result-pruner": "工具长文本输出智能修剪与折叠优化",

      // 核心工具套件
      "tools": "系统工具注册中心与执行协议路由",
      "tool-bash": "工作区 Bash 终端原生命令执行工具",
      "tool-bash-persistent": "长驻有状态 Bash 终端会话管理工具",
      "tool-pwsh": "PowerShell 终端命令执行工具 (Windows)",
      "tool-fs": "文件系统检视、文件读写与目录树遍历",
      "tool-fs-search": "代码库与工程文件快速关键词检索",
      "tool-str-replace-editor": "精准文本替换与源代码结构化编辑器",
      "tool-web": "网络搜索与实时网页正文抓取解析",
      "web-search-deepseek": "DeepSeek 官方搜索 API 接入驱动",
      "tool-subagent": "派发子任务给具备独立上下文的子智能体",
      "tool-subagent-control": "后台子智能体协作交互与执行控制",
      "tool-subagent-report": "子智能体成果汇总与双向汇报通道",
      "tool-todo": "结构化待办清单拆解与进度实时跟踪",
      "tool-workflow": "自动化多步骤业务工作流执行工具",
      "tool-ralph": "智能体辅助增强工具套件",
      "tool-cordis": "Cordis 内部服务动态调用与调试工具",
      "tool-ask-user": "主动向用户提问澄清需求的交互工具",
      "tool-call-timeout-policy": "工具调用超时保护与熔断恢复策略",
      "repeat-tool-reminder": "重复工具调用死循环检测与提醒拦截",

      // MCP 与模块契约
      "mcp-client": "Model Context Protocol (MCP) 客户端连接器",
      "typert-registry": "TypeRT 运行时类型系统与契约注册中心",
      "typert-loader": "TypeRT 模块动态加载与跨语言绑定",
      "typert-protocol": "TypeRT IPC 通信协议与数据编解码",

      // 安全沙箱与权限
      "sandbox": "安全沙箱隔离核心环境",
      "sandbox-local": "本地受限隔离运行环境",
      "sandbox-policy": "沙箱权限与文件/网络隔离策略",
      "sandbox-windows-acl": "Windows 原生 ACL 安全权限控制",
      "node-addon-landlock-run": "Linux Landlock 内核级无特权沙箱隔离",
      "user-approval": "敏感操作（修改文件/执行命令）人工审批机制",
      "permission-presets": "权限预设策略组合（只读/审批/完全访问）",

      // 前端交互与 UI 模块
      "client-web": "官方 Web 客户端页面外壳容器",
      "client-web-react": "基于 React 的现代化前端渲染引擎",
      "client-locale": "客户端多语言国际化语言包管理",
      "client-runtime": "客户端运行时服务容器与状态订阅",
      "client-connection": "客户端与后端 WebSocket/IPC 实时连接",
      "client-hmr": "客户端 UI 组件热重载更新支持",
      "client-modules": "客户端动态模块管理器",
      "client-schema-form": "动态配置项表单自动渲染组件",
      "ui-commands": "/ 快捷命令弹窗与键盘交互控制器",
      "ui-conversation": "对话流渲染、Markdown 解析与代码高亮",
      "ui-layout": "现代化多栏布局与响应式工作区分割",
      "ui-sidebar": "侧边栏工作区与历史会话导航面板",
      "ui-primitives": "基础 UI 原子组件库与图标集",
      "ui-settings": "系统偏好与运行时设置主面板",
      "ui-settings-general": "常规基础设置（语言/更新/端口）",
      "ui-settings-models": "大模型服务商与 API Key 配置面板",
      "ui-settings-plugins": "插件管理与自定义配置页面",
      "ui-settings-plugin-inventory": "插件清单与全量模块状态看板",
      "ui-agent-preset": "智能体角色预设选择器",
      "ui-attachment": "附件上传、图片识别与文件拖拽管理",
      "ui-cordis": "Cordis 调试控制台与插件树可视化",
      "ui-deliverables": "智能体执行交付物汇总呈现面板",
      "ui-directory-picker-browse": "网页端目录浏览与选择器",
      "ui-directory-picker-native": "系统原生文件夹选择弹窗集成",
      "ui-goal": "长任务目标设定与执行进度看板",
      "ui-input-trigger": "输入框唤醒与快捷触发器",
      "ui-jobs": "后台作业监控与状态展示面板",
      "ui-message-feedback": "单条消息满意度反馈评分组件",
      "ui-model-selection": "顶部大模型切换下拉选择组件",
      "ui-permission-presets": "输入区权限模式切换与审批提示",
      "ui-plan": "架构规划模式步骤与任务看板",
      "ui-skill": "技能挂载、徽章与技能管理面板",
      "ui-slots": "UI 插槽扩展与组件注入通道",
      "ui-subagent": "子智能体协同界面与状态卡片",
      "ui-theme": "深浅主题自适应与设计规范管理",
      "ui-tool": "工具调用卡片与执行结果富文本折叠",
      "ui-trajectory": "AI 思考链与推理轨迹流式展示",
      "ui-user-questions": "人机交互提问弹窗与输入收集",
      "ui-workflow-run": "工作流运行监控与进度跟踪面板",
      "ui-workspace": "工作区工程目录与文件树管理",

      // 后端服务与底层运行时
      "host-webserver": "本地 Web HTTP 服务器 (默认端口 3080)",
      "host-frontend-static": "静态前端资源托管与打包服务",
      "host-apiproxy": "跨域 API 网关与请求转发代理",
      "host-directory-picker": "系统文件夹选择服务",
      "host-directory-picker-auto": "自动目录选择探测适配器",
      "host-directory-picker-browse": "浏览模式目录选择器",
      "host-directory-picker-native": "原生目录选择器后端服务",
      "host-plugin-inventory": "插件清单与运行时状态采集服务",
      "api-gateway": "统一 API 网关与路由鉴权分发",
      "api-remotes": "远程 RPC 服务连接与双向通信通道",
      "app-boot": "应用启动编排、初始化检查与环境自检",
      "atomic-write": "原子安全文件写入（写入临时文件+重命名覆盖）",
      "attachment": "通用附件管理与多模态数据处理",
      "attachment-local": "本地附件存储与缓存驱动",
      "anonymous-user-id": "匿名用户标识生成与持久化",
      "base": "DeepSeek Harness 核心基础契约",
      "bash-local": "本地 Bash 终端底层执行器",
      "bash-sandbox": "沙箱隔离 Bash 终端执行器",
      "brand": "DeepSeek Harness 品牌标识与视觉规范",
      "cmdline": "CLI 命令行参数解析与命令分发",
      "code-runtime": "代码执行沙箱环境与运行时隔离",
      "code-runtime-worker-thread": "基于 Worker 线程的代码执行隔离驱动",
      "command-compact": "compact 历史压缩命令实现",
      "command-feedback": "feedback 会话反馈命令实现",
      "command-goal": "goal 目标管理命令实现",
      "commands": "命令注册管理系统",
      "cordis-client-runner": "客户端 Cordis 容器运行环境",
      "cordis-host-runner": "服务端 Cordis 容器运行环境",
      "credentials": "敏感凭据（API Key / Token）安全加密存储",
      "credentials-local": "本地凭据存储适配器",
      "fs": "文件系统核心抽象服务",
      "fs-local": "本地文件系统直接读写驱动",
      "fs-observation-policy": "文件变更监听与安全观察策略",
      "fs-sandbox": "文件系统沙箱隔离访问策略",
      "goal": "长任务目标管理核心服务",
      "goal-round-driver": "多轮目标驱动与收敛控制器",
      "headless": "无头模式 (Headless) 运行支撑",
      "home-paths": "$DSH_HOME 用户目录与数据路径管理",
      "invariants": "系统不变量与架构安全校验器",
      "jobs": "后台异步作业调度与任务队列管理器",
      "jobs-local": "本地作业进程池监管与执行",
      "launch-environment": "运行环境变量探测与虚拟环境隔离",
      "message-feedback": "消息质量反馈收集与处理",
      "native-command": "系统原生命令调用驱动",
      "output-retention": "命令与工具输出保留与截断策略",
      "pwsh-local": "PowerShell 本地执行驱动",
      "pwsh-sandbox": "PowerShell 沙箱执行驱动",
      "schedule": "定时计划与定时任务调度器",
      "scope": "Cordis 依赖上下文生命周期作用域",
      "settings": "系统偏好配置管理服务",
      "settings-file": "settings.yaml 配置文件读写校验",
      "shell": "终端 Shell 会话抽象接口",
      "shell-env": "Shell 环境变量管理与传递",
      "skill": "技能 (Skill) 挂载与能力扩展框架",
      "skill-badge": "技能状态徽章与展示标识",
      "skill-filesystem": "基于文件系统的本地技能加载器",
      "spill": "内存大对象磁盘溢出存储策略",
      "spill-local": "本地磁盘溢出存储驱动",
      "spill-policy": "大文本/超长上下文溢出安全策略",
      "storage": "数据持久化存储主服务",
      "storage-domain": "多域存储数据隔离管理",
      "storage-json": "基于 JSON 格式的数据存储驱动",
      "subagent": "多智能体协作与子代理管理系统",
      "subagent-fork-in-process": "进程内快速 Fork 子智能体驱动",
      "subagent-in-process-driver": "进程内子智能体执行器",
      "subagent-spawn-in-process": "独立上下文子智能体派生驱动",
      "subprocess": "子进程安全派生与标准输入输出管理",
      "subprocess-local": "本地子进程直接派生执行驱动",
      "terminal": "虚拟终端会话与 PTY 交互管理",
      "terminal-bash": "Bash 专用终端仿真驱动",
      "time-context": "时间上下文与时区动态同步",
      "timeout": "全局操作超时保护与熔断控制器",
      "tmux-context": "Tmux 会话多路复用上下文绑定",
      "workflow": "自动化业务工作流执行引擎",
      "workflow-worker-thread": "基于独立 Worker 线程的工作流执行器",
      "workspace": "智能体工程工作区管理服务"
    };

    function getPluginDescription(shortName, fullModule) {
      if (PLUGIN_DICT[shortName]) return PLUGIN_DICT[shortName];
      if (PLUGIN_DICT[fullModule]) return PLUGIN_DICT[fullModule];

      var s = (shortName || '').toLowerCase();
      if (s.startsWith('tool-')) return '专用功能工具扩展 (' + s.replace(/^tool-/, '') + ')';
      if (s.startsWith('llm-')) return '大语言模型适配驱动 (' + s.replace(/^llm-/, '') + ')';
      if (s.startsWith('mcp-')) return 'Model Context Protocol 外部协议扩展 (' + s.replace(/^mcp-/, '') + ')';
      if (s.startsWith('skill-')) return '智能体技能扩展 (' + s.replace(/^skill-/, '') + ')';
      if (s.startsWith('sandbox-')) return '安全沙箱隔离模块 (' + s.replace(/^sandbox-/, '') + ')';
      if (s.startsWith('ui-') || s.startsWith('client-ui-')) return '前端界面与交互组件 (' + s.replace(/^(?:client-)?ui-/, '') + ')';
      if (s.startsWith('session-')) return '会话数据与生命周期组件 (' + s.replace(/^session-/, '') + ')';
      if (s.startsWith('cordis-plugin-')) return 'Cordis 微内核扩展插件 (' + s.replace(/^cordis-plugin-/, '') + ')';

      return 'DeepSeek Harness 模块扩展 (' + shortName + ')';
    }

    // 注入插件列表卡片的美化样式
    function injectPluginCustomStyles() {
      if (document.getElementById('dsh-plugin-i18n-style')) return;
      var style = document.createElement('style');
      style.id = 'dsh-plugin-i18n-style';
      style.textContent = `
        .dsh-plugin-title-box {
          display: flex !important;
          flex-direction: column !important;
          align-items: flex-start !important;
          gap: 2px !important;
          min-width: 0 !important;
          text-align: left !important;
          max-width: 100% !important;
        }
        .dsh-plugin-name {
          font-size: 13.5px !important;
          font-weight: 600 !important;
          color: var(--dsw-alias-label-primary, #111827) !important;
          font-family: var(--ds-font-family-code, ui-monospace, monospace) !important;
          line-height: 18px !important;
          overflow: hidden !important;
          text-overflow: ellipsis !important;
          white-space: nowrap !important;
          max-width: 100% !important;
        }
        .dsh-plugin-desc-tag {
          font-size: 11px !important;
          font-weight: 400 !important;
          color: var(--dsw-alias-label-tertiary, #64748b) !important;
          line-height: 15px !important;
          overflow: hidden !important;
          text-overflow: ellipsis !important;
          white-space: nowrap !important;
          max-width: 100% !important;
          letter-spacing: -0.01em !important;
        }
        [class*="cardContent"] {
          min-height: 60px !important;
          padding: 10px 14px !important;
        }
        [class*="cardTitle"] {
          overflow: visible !important;
          min-width: 0 !important;
          flex: 1 1 auto !important;
        }
        .dsh-plugin-detail-custom {
          display: contents !important;
        }
        .dsh-plugin-detail-custom dt {
          color: var(--dsw-alias-label-tertiary, #64748b) !important;
          font-size: 11px !important;
          line-height: 17px !important;
        }
        .dsh-plugin-detail-custom dd {
          color: var(--dsw-alias-label-primary, #111827) !important;
          font-size: 12px !important;
          line-height: 17px !important;
          margin: 0 !important;
        }
      `;
      document.head.appendChild(style);
    }

    function decoratePluginCards() {
      injectPluginCustomStyles();
      var cards = document.querySelectorAll('li[data-plugin-entry]');
      if (!cards || cards.length === 0) return;

      for (var i = 0; i < cards.length; i++) {
        var card = cards[i];
        var titleEl = card.querySelector('strong');
        if (!titleEl) continue;

        if (titleEl.querySelector('.dsh-plugin-desc-tag')) continue;

        var fullModule = titleEl.getAttribute('title') || card.getAttribute('data-plugin-entry') || '';
        var rawText = titleEl.textContent.trim();
        if (!rawText) continue;

        var desc = getPluginDescription(rawText, fullModule);

        titleEl.innerHTML = '';
        var box = document.createElement('div');
        box.className = 'dsh-plugin-title-box';

        var nameSpan = document.createElement('span');
        nameSpan.className = 'dsh-plugin-name';
        nameSpan.textContent = rawText;

        var descSpan = document.createElement('span');
        descSpan.className = 'dsh-plugin-desc-tag';
        descSpan.textContent = desc;
        descSpan.title = desc;

        box.appendChild(nameSpan);
        box.appendChild(descSpan);
        titleEl.appendChild(box);

        // 展开详情页追加详细说明
        var details = card.querySelector('dl');
        if (details && !details.querySelector('.dsh-plugin-detail-custom')) {
          var dDiv = document.createElement('div');
          dDiv.className = 'dsh-plugin-detail-custom';
          
          var dt = document.createElement('dt');
          dt.textContent = '功能说明';
          var dd = document.createElement('dd');
          dd.textContent = desc;
          
          dDiv.appendChild(dt);
          dDiv.appendChild(dd);
          details.appendChild(dDiv);
        }
      }
    }

    try {
      var observer = new MutationObserver(function(mutations) {
        for (var i = 0; i < mutations.length; i++) {
          var m = mutations[i];
          if (m.type === 'childList') {
            for (var j = 0; j < m.addedNodes.length; j++) {
              translateNode(m.addedNodes[j]);
            }
            decoratePluginCards();
          } else if (m.type === 'characterData') {
            translateNode(m.target);
          }
        }
      });
      observer.observe(document.body || document.documentElement, {
        childList: true,
        subtree: true,
        characterData: true
      });
      translateNode(document.body);
      decoratePluginCards();
    } catch (e) {}

    setInterval(function() {
      try {
        translateNode(document.body);
        decoratePluginCards();
      } catch (e) {}
    }, 800);

    window.__dshI18nInstalled = true;
  }

  // ── 4. 对话与界面中超链接与URL代码块系统浏览器打开拦截器 ──────────
  if (!window.__dshLinkInterceptorInstalled) {
    function extractUrl(str) {
      if (!str || typeof str !== 'string') return null;
      var trimmed = str.trim();
      if (/^https?:\/\/[^\s"'<>]+$/i.test(trimmed) || /^mailto:[^\s]+$/i.test(trimmed)) {
        return trimmed;
      }
      return null;
    }

    function openExternally(url) {
      if (!url || typeof url !== 'string') return;
      var u = url.trim();
      if (!/^https?:\/\//i.test(u) && !/^mailto:/i.test(u)) return;

      // 1. 优先调用 Tauri Core Invoke
      if (window.__TAURI__ && window.__TAURI__.core && typeof window.__TAURI__.core.invoke === 'function') {
        window.__TAURI__.core.invoke('open_external_url', { url: u }).catch(function(err) {
          console.error('[dsh-client] open_external_url via core failed:', err);
        });
        return;
      }

      // 2. 尝试 __TAURI_INTERNALS__ Invoke (Tauri 2 内部通信通道)
      if (window.__TAURI_INTERNALS__ && typeof window.__TAURI_INTERNALS__.invoke === 'function') {
        window.__TAURI_INTERNALS__.invoke('open_external_url', { url: u }).catch(function(err) {
          console.error('[dsh-client] open_external_url via internals failed:', err);
        });
        return;
      }

      // 3. 尝试 __TAURI__.opener
      if (window.__TAURI__ && window.__TAURI__.opener && typeof window.__TAURI__.opener.openUrl === 'function') {
        window.__TAURI__.opener.openUrl(u).catch(function(){});
        return;
      }

      // 4. 兜底
      try {
        window.open(u, '_blank', 'noopener,noreferrer');
      } catch (e) {}
    }

    // 注入 URL 代码块的可交互样式
    function injectUrlBlockStyles() {
      if (document.getElementById('dsh-url-interactive-style')) return;
      var style = document.createElement('style');
      style.id = 'dsh-url-interactive-style';
      style.textContent = `
        .dsh-clickable-url {
          cursor: pointer !important;
          transition: all 0.15s ease !important;
          text-decoration: underline !important;
          text-underline-offset: 2px !important;
        }
        .dsh-clickable-url:hover {
          filter: brightness(1.15) !important;
          opacity: 0.9 !important;
        }
      `;
      document.head.appendChild(style);
    }

    // 扫描页面中的 URL 代码块（如 <code>http://127.0.0.1:3080</code>）增强交互
    function scanAndDecorateUrlBlocks() {
      injectUrlBlockStyles();
      var codeEls = document.querySelectorAll('code, span');
      for (var i = 0; i < codeEls.length; i++) {
        var el = codeEls[i];
        if (el.children.length === 0) {
          var u = extractUrl(el.textContent);
          if (u && !el.classList.contains('dsh-clickable-url')) {
            el.classList.add('dsh-clickable-url');
            el.title = '点击在默认浏览器中打开: ' + u;
          }
        }
      }
    }

    // 全局捕获阶段拦截所有 <a> 标签与包含 URL 的代码块/文本点击
    document.addEventListener('click', function(e) {
      var target = e.target;
      if (!target) return;

      // 1. 检查是否为 <a> 超链接
      var a = target.closest ? target.closest('a') : null;
      if (a) {
        var href = a.getAttribute('href') || a.href;
        var urlFromHref = extractUrl(href);
        if (urlFromHref) {
          e.preventDefault();
          e.stopPropagation();
          openExternally(urlFromHref);
          return;
        }
      }

      // 2. 检查是否为包含 URL 的代码块或文本元素（如 <code>http://127.0.0.1:3080</code>）
      var text = target.textContent ? target.textContent.trim() : '';
      var urlFromText = extractUrl(text);
      if (urlFromText) {
        e.preventDefault();
        e.stopPropagation();
        openExternally(urlFromText);
        return;
      }
    }, true);

    // 拦截 window.open 弹窗调用
    var rawOpen = window.open;
    window.open = function(url) {
      var u = extractUrl(url);
      if (u) {
        openExternally(u);
        return null;
      }
      return rawOpen ? rawOpen.apply(this, arguments) : null;
    };

    setInterval(scanAndDecorateUrlBlocks, 1000);
    scanAndDecorateUrlBlocks();

    window.__dshLinkInterceptorInstalled = true;
  }
})()"#;

/// 定期向主窗口注入运行时能力（剪贴板桥 + 中文注释引擎，幂等）。
fn spawn_bridge_injector(app: AppHandle) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(1500));
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.eval(RUNTIME_INJECTOR_SCRIPT);
        }
    });
}

// ── commands (console page) ─────────────────────────────────────────────────

#[tauri::command]
fn status(state: tauri::State<'_, AppState>) -> server::ServerStatus {
    state.server.status()
}

#[tauri::command]
fn logs(state: tauri::State<'_, AppState>, n: Option<usize>) -> Vec<server::LogLine> {
    state.server.logs(n.unwrap_or(200))
}

#[tauri::command]
fn clear_logs(state: tauri::State<'_, AppState>) {
    state.server.clear_logs();
}

#[tauri::command]
fn start(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let srv = state.server.clone();
    thread::spawn(move || {
        let _ = srv.start();
    });
    Ok(())
}

#[tauri::command]
fn restart(state: tauri::State<'_, AppState>) -> Result<(), String> {
    restart_server(&state.server);
    Ok(())
}

#[tauri::command]
fn stop(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.server.stop();
    Ok(())
}

#[tauri::command]
fn update_dsh(state: tauri::State<'_, AppState>, version: String) -> Result<(), String> {
    let srv = state.server.clone();
    thread::spawn(move || {
        let _ = srv.update_dsh(&version);
    });
    Ok(())
}

#[derive(serde::Serialize)]
struct Info {
    version: String,
    dsh_version: Option<String>,
    data_dir: String,
    port: u16,
}

#[tauri::command]
fn info(_app: AppHandle, state: tauri::State<'_, AppState>) -> Info {
    let dsh_version = dsh_version(&state.server);
    let data_dir = server::dsh_data_dir();
    Info {
        version: env!("CARGO_PKG_VERSION").to_string(),
        dsh_version,
        data_dir: data_dir.display().to_string(),
        port: DshServer::effective_port(),
    }
}

/// 自定义服务端口（持久化到桌面配置并重启服务生效）。
#[tauri::command]
fn set_port(state: tauri::State<'_, AppState>, port: u16) -> Result<(), String> {
    if !(1024..=65535).contains(&port) {
        return Err("端口需在 1024-65535 之间".into());
    }
    if port == DshServer::effective_port() {
        return Ok(());
    }
    server::write_settings_port(port)?;
    restart_server(&state.server);
    Ok(())
}

/// 终止占用指定端口的进程并重新拉起 dsh 服务
#[tauri::command]
fn kill_port_process(state: tauri::State<'_, AppState>, port: Option<u16>) -> Result<(), String> {
    let p = port.unwrap_or_else(DshServer::effective_port);
    let srv = state.server.clone();
    thread::spawn(move || {
        let _ = srv.kill_and_restart(p);
    });
    Ok(())
}

#[derive(serde::Serialize)]
struct OfficialDshInfo {
    current_version: Option<String>,
    latest_version: Option<String>,
    has_update: bool,
    npm_url: String,
    github_url: String,
}

#[tauri::command]
fn check_official_dsh(state: tauri::State<'_, AppState>) -> OfficialDshInfo {
    let current_version = dsh_version(&state.server);
    let latest_version = DshServer::fetch_latest_dsh_version();
    let has_update = match (&current_version, &latest_version) {
        (Some(cur), Some(latest)) => cur != latest,
        _ => false,
    };
    OfficialDshInfo {
        current_version,
        latest_version,
        has_update,
        npm_url: "https://www.npmjs.com/package/@deepseek-ai/dsh".into(),
        github_url: "https://github.com/deepseek-ai/deepseek-harness".into(),
    }
}

/// 手动接管指定端口已运行的服务
#[tauri::command]
fn attach_port(state: tauri::State<'_, AppState>, port: Option<u16>) -> Result<(), String> {
    let p = port.unwrap_or_else(DshServer::effective_port);
    state.server.attach_port(p)
}

#[tauri::command]
fn open_official_repo(app: AppHandle) -> Result<(), String> {
    app.opener().open_url("https://github.com/deepseek-ai/deepseek-harness", None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_browser(app: AppHandle) -> Result<(), String> {
    let port = DshServer::effective_port();
    let url = format!("http://127.0.0.1:{port}");
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_external_url(app: AppHandle, url: String) -> Result<(), String> {
    let u = url.trim();
    if u.starts_with("http://") || u.starts_with("https://") || u.starts_with("mailto:") || u.starts_with("tel:") {
        app.opener().open_url(u, None::<&str>).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 手动进入官方 dsh 界面（唯一的进入入口：应用打开后停留在控制台，
/// 由用户点击「进入界面」后由后端权威导航，服务未运行时拒绝）。
#[tauri::command]
fn enter_ui(app: AppHandle, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let status = state.server.status();
    if status.state != "running" {
        return Err("服务未运行，请先启动服务".into());
    }
    let port = status.port.unwrap_or_else(DshServer::effective_port);
    navigate_to(&app, &format!("http://127.0.0.1:{port}"));
    Ok(())
}

#[tauri::command]
fn get_workspace() -> String {
    server::effective_workspace().display().to_string()
}

#[tauri::command]
fn set_workspace(state: tauri::State<'_, AppState>, path: String) -> Result<(), String> {
    let p = path.trim();
    if p.is_empty() {
        return Err("工作区路径不能为空".into());
    }
    let pb = PathBuf::from(p);
    if !pb.is_dir() {
        return Err(format!("指定路径不存在或非目录: {p}"));
    }
    server::write_workspace(Some(p))?;
    restart_server(&state.server);
    Ok(())
}

#[tauri::command]
fn reset_workspace(state: tauri::State<'_, AppState>) -> Result<(), String> {
    server::write_workspace(None)?;
    restart_server(&state.server);
    Ok(())
}

#[tauri::command]
fn pick_workspace_dir() -> Result<Option<String>, String> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("osascript")
            .args(["-e", "POSIX path of (choose folder with prompt \"选择 DeepSeek Harness 工作区目录\")"])
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(Some(path));
            }
        }
        return Ok(None);
    }
    #[cfg(target_os = "windows")]
    {
        let script = "[System.Reflection.Assembly]::LoadWithPartialName('System.windows.forms') | Out-Null; $f = New-Object System.Windows.Forms.FolderBrowserDialog; if ($f.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { Write-Output $f.SelectedPath }";
        let output = Command::new("powershell")
            .args(["-Command", script])
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(Some(path));
            }
        }
        return Ok(None);
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = Command::new("zenity").args(["--file-selection", "--directory"]).output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Ok(Some(path));
                }
            }
        }
        return Ok(None);
    }
}

#[tauri::command]
fn open_workspace_dir(app: AppHandle) -> Result<(), String> {
    let dir = server::effective_workspace();
    app.opener().open_path(dir.display().to_string(), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_session_stats() -> server::SessionStats {
    server::get_session_stats()
}

#[tauri::command]
fn backup_sessions(app: AppHandle) -> Result<String, String> {
    let backup_path = server::backup_sessions()?;
    let backups_dir = server::dsh_data_dir().join("backups");
    let _ = app.opener().reveal_item_in_dir(backups_dir);
    Ok(backup_path.display().to_string())
}

#[tauri::command]
fn clear_session_cache() -> Result<String, String> {
    server::clear_session_cache()
}

#[tauri::command]
fn open_data_dir(app: AppHandle) -> Result<(), String> {
    let dir = server::dsh_data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // 收紧数据目录权限（审计 M13）：避免其他本地用户枚举
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    app.opener().reveal_item_in_dir(dir).map_err(|e| e.to_string())
}

#[tauri::command]
fn autostart_enabled(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn autostart_set(app: AppHandle, enabled: bool) -> Result<(), String> {
    if enabled {
        app.autolaunch().enable().map_err(|e| e.to_string())
    } else {
        app.autolaunch().disable().map_err(|e| e.to_string())
    }
}

// ── 界面语言（官方 locale 设置，写 ~/.dsh/settings.yaml）─────────────────────

fn settings_path() -> PathBuf {
    server::dsh_data_dir().join("settings.yaml")
}

fn read_settings_doc() -> serde_yaml::Value {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|t| serde_yaml::from_str(&t).ok())
        .unwrap_or_else(|| serde_yaml::Value::Mapping(Default::default()))
}

fn write_settings_doc(doc: &serde_yaml::Value) -> Result<(), String> {
    let path = settings_path();
    let dir = path.parent().ok_or("settings 路径无父目录")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let text = serde_yaml::to_string(doc).map_err(|e| e.to_string())?;
    // 原子写（临时文件 + rename），与官方 dsh-atomic-write 一致，避免半成品
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

/// 读取界面语言偏好："zh" / "en" / "auto"（未设置时跟随浏览器）。
#[tauri::command]
fn get_locale() -> String {
    read_settings_doc()
        .get("locale")
        .and_then(|l| l.get("preference"))
        .and_then(|p| p.as_str())
        .unwrap_or("auto")
        .to_string()
}

/// 写入官方 locale.preference（zh / en / auto）。官方 dsh 通过 chokidar
/// watch settings.yaml，外部修改会被加载；部分界面重启服务后完全生效。
#[tauri::command]
fn set_locale(preference: String) -> Result<(), String> {
    if !matches!(preference.as_str(), "zh" | "en" | "auto") {
        return Err("preference 必须为 zh / en / auto".into());
    }
    let mut doc = read_settings_doc();
    let map = doc
        .as_mapping_mut()
        .ok_or("settings.yaml 顶层不是映射")?;
    let locale = map
        .entry(serde_yaml::Value::String("locale".into()))
        .or_insert_with(|| serde_yaml::Value::Mapping(Default::default()));
    if let serde_yaml::Value::Mapping(lm) = locale {
        lm.insert(
            serde_yaml::Value::String("preference".into()),
            serde_yaml::Value::String(preference.clone()),
        );
    }
    write_settings_doc(&doc)?;
    Ok(())
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Read the installed dsh package version from the bundled runtime
/// (user-updated runtime first, bundle fallback).
fn dsh_version(server: &DshServer) -> Option<String> {
    let pkg = server
        .runtime_paths_for_version()
        .into_iter()
        .find_map(|base| {
            let p = base.join("node_modules").join("@deepseek-ai").join("dsh").join("package.json");
            p.is_file().then_some(p)
        })?;
    let text = std::fs::read_to_string(pkg).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("version")?.as_str().map(|s| s.to_string())
}

fn restart_server(server: &DshServer) {
    let srv = server.clone();
    thread::spawn(move || {
        srv.stop();
        thread::sleep(Duration::from_millis(600));
        let _ = srv.start();
    });
}

fn show_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

/// 确保远程页剪贴板写能力覆盖当前端口：默认 3080 由静态 capability 覆盖；
/// 自定义/自动切换的端口通过 dynamic-acl 运行时授权（审计 H6）。
fn ensure_port_capability(app: &AppHandle, port: u16) {
    if port == server::DEFAULT_PORT {
        return;
    }
    use tauri::ipc::CapabilityBuilder;
    let cap = CapabilityBuilder::new("dsh-page-custom")
        .local(false)
        .window("main")
        .remote(format!("http://127.0.0.1:{port}/*"))
        .permission("clipboard-manager:allow-write-text")
        .permission("allow-open-external-url");
    if let Err(e) = app.add_capability(cap) {
        eprintln!("failed to add custom-port capability: {e}");
    }
}

fn navigate_to(app: &AppHandle, url: &str) {
    if let Some(win) = app.get_webview_window("main") {
        if let Ok(parsed) = url.parse() {
            let _ = win.navigate(parsed);
        }
    }
}

fn handle_menu_action(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    match id {
        menu::MENU_OPEN_BROWSER => {
            let _ = open_browser(app.clone());
        }
        menu::MENU_OPEN_DATA_DIR => {
            let _ = open_data_dir(app.clone());
        }
        menu::MENU_ENTER_UI => {
            if let Err(e) = enter_ui(app.clone(), state) {
                eprintln!("enter_ui failed: {e}");
            }
        }
        menu::MENU_RESTART => restart_server(&state.server),
        menu::MENU_CONSOLE => {
            if let Some(url) = state.console_url.lock().unwrap().clone() {
                navigate_to(app, &url);
            }
        }
        menu::MENU_RELOAD => {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.eval("window.location.reload()");
            }
        }
        menu::MENU_SHOW_WINDOW => show_main_window(app),
        menu::MENU_QUIT => {
            state.server.stop();
            app.exit(0);
        }
        _ => {}
    }
}

/// Watch the server state and move the webview back to the console page
/// whenever the service leaves the running state (no dead pages). Entering
/// the dsh UI is always MANUAL: the console「进入界面」button (enter_ui)
/// is the only entry point — the webview is never auto-navigated into the
/// remote page.
fn spawn_navigation_watcher(app: AppHandle, state: AppState) {
    thread::spawn(move || {
        let mut last = ServerState::Stopped;
        loop {
            let current = state.server.status();
            let cur_state = match current.state.as_str() {
                "running" => ServerState::Running,
                "starting" => ServerState::Starting,
                "error" => ServerState::Error,
                _ => ServerState::Stopped,
            };
            if cur_state != last {
                // 服务离开运行态（停止/重启/崩溃）：回到控制台，避免连接拒绝死页
                if matches!(last, ServerState::Running | ServerState::Starting)
                    && cur_state != ServerState::Running
                {
                    if let Some(url) = state.console_url.lock().unwrap().clone() {
                        navigate_to(&app, &url);
                    }
                }
                last = cur_state;
            }
            thread::sleep(Duration::from_millis(800));
        }
    });
}

// ── entry ───────────────────────────────────────────────────────────────────

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(AppState {
            server: DshServer::new(),
            console_url: Mutex::new(None),
            hide_notice_shown: AtomicBool::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            status,
            logs,
            clear_logs,
            start,
            restart,
            stop,
            update_dsh,
            info,
            set_port,
            kill_port_process,
            attach_port,
            check_official_dsh,
            open_official_repo,
            open_browser,
            open_external_url,
            enter_ui,
            open_data_dir,
            autostart_enabled,
            autostart_set,
            get_workspace,
            set_workspace,
            reset_workspace,
            pick_workspace_dir,
            open_workspace_dir,
            get_session_stats,
            backup_sessions,
            clear_session_cache,
            get_locale,
            set_locale
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // remember the console URL so we can navigate back to it;
            // the webview may not have committed its first load yet, so
            // capture it shortly after startup as well (audit H11).
            {
                if let Some(win) = app.get_webview_window("main") {
                    if let Ok(url) = win.url() {
                        let s = url.to_string();
                        if !s.is_empty() && s != "about:blank" {
                            app.state::<AppState>()
                                .console_url
                                .lock()
                                .unwrap()
                                .replace(s);
                        }
                    }
                }
            }
            let handle2 = handle.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(1500));
                if let Some(win) = handle2.get_webview_window("main") {
                    if let Ok(url) = win.url() {
                        let s = url.to_string();
                        if !s.is_empty() && s != "about:blank" {
                            handle2
                                .state::<AppState>()
                                .console_url
                                .lock()
                                .unwrap()
                                .replace(s);
                        }
                    }
                }
            });

            // bundled runtime location (resources/runtime)
            if let Ok(dir) = app.path().resource_dir() {
                app.state::<AppState>().server.set_resource_dir(dir);
            }

            // 端口能力联动：启动时按有效端口授权；运行时自动换端口时动态授权
            ensure_port_capability(&handle, DshServer::effective_port());
            {
                let state = app.state::<AppState>();
                let cb_handle = handle.clone();
                state.server.on_port_change(Box::new(move |port| {
                    let app = cb_handle.clone();
                    thread::spawn(move || ensure_port_capability(&app, port));
                }));
            }

            app.set_menu(menu::build_menu(&handle)?)?;
            menu::build_tray(&handle, handle_menu_action)?;

            let state = app.state::<AppState>();
            spawn_navigation_watcher(handle.clone(), state.inner().clone());
            spawn_bridge_injector(handle.clone());

            // auto-start the harness service shortly after the window appears
            let srv = state.server.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(400));
                let _ = srv.start();
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { .. } = event {
                // 关闭主窗口时彻底终止服务并退出应用，释放端口和进程
                let state = window.state::<AppState>();
                state.server.stop();
                window.app_handle().exit(0);
            }
        })
        .on_menu_event(|app, event| handle_menu_action(app, event.id.as_ref()))
        .build(tauri::generate_context!())
        .expect("error while building deepseek-harness-desktop-client")
        .run(|app_handle, event| {
            if let tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit = event {
                let state = app_handle.state::<AppState>();
                state.server.stop();
            }
        });
}
