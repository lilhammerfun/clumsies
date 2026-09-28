import { defineConfig } from "vitepress";
import footnote from "markdown-it-footnote";
import container from "markdown-it-container";
import { withMermaid } from "vitepress-plugin-mermaid";

export default withMermaid(
  defineConfig({
    lang: "en-US",
    title: "clumsies",
    description: "Use shared Memory with coding agents, understand the design, and find Clumsies integration and maintenance references.",
    base: "/",
    // Chinese pages kept for history only; they are not part of the current locale
    // contract and are excluded from the build so their old links cannot break it.
    srcExclude: ["zh/archive/**"],
    head: [["link", { rel: "icon", type: "image/png", href: "/logo.png" }]],
    appearance: true,
    cleanUrls: true,
    lastUpdated: true,
    locales: {
      root: { label: "English", lang: "en-US" },
      zh: {
        label: "中文",
        lang: "zh-CN",
        link: "/zh/",
        themeConfig: {
          outline: {
            level: [2, 3],
            label: "本页目录",
          },
          nav: [
            { text: "开始阅读", link: "/zh/" },
            { text: "任务指南", link: "/zh/guides/" },
            { text: "概念与设计", link: "/zh/overview" },
            { text: "接口参考", link: "/zh/reference/" },
            { text: "开发维护", link: "/zh/repos" },
          ],
          sidebar: [
            {
              text: "初识与快速开始",
              collapsed: false,
              items: [
                { text: "阅读路线", link: "/zh/" },
                { text: "快速开始说明", link: "/zh/quickstart/" },
                { text: "获取 App", link: "/zh/quickstart/install" },
                { text: "连接组织", link: "/zh/quickstart/connect" },
                { text: "1. 创建项目", link: "/zh/quickstart/create-project" },
                { text: "2. 选择 Memory", link: "/zh/quickstart/select-memory" },
                { text: "3. 让 Codex 使用", link: "/zh/quickstart/use-with-agent" },
                { text: "4. 提出修改", link: "/zh/quickstart/update-memory" },
                { text: "5. 审阅并发布", link: "/zh/quickstart/review-and-publish" },
              ]
            },
            {
              text: "任务指南",
              collapsed: true,
              items: [
                { text: "按任务查找", link: "/zh/guides/" },
                { text: "常用成员任务", link: "/zh/guides/how-to-use-clumsies" },
                { text: "创建新的 Memory", link: "/zh/guides/create-memory" },
                { text: "记忆维护规范", link: "/zh/guides/memory-guidelines" },
                { text: "接入 Agent", link: "/zh/guides/agent-runtime" },
                { text: "部署组织服务", link: "/zh/guides/deploy-for-an-org" },
                { text: "排查问题", link: "/zh/guides/troubleshooting" },
                { text: "接入 DeepSeek Harness", link: "/zh/guides/dsh-integration" },
              ]
            },
            {
              text: "概念与设计",
              collapsed: true,
              items: [
                { text: "认识 Clumsies", link: "/zh/overview" },
                { text: "系统架构", link: "/zh/architecture" },
                { text: "核心数据结构", link: "/zh/data-model" },
                { text: "完整流程", link: "/zh/flows" },
                { text: "术语表", link: "/zh/glossary" },
                {
                  text: "子系统详解",
                  collapsed: true,
                  items: [
                    { text: "Organization Memory", link: "/zh/artifact" },
                    { text: "Project 选择与绑定", link: "/zh/workspace" },
                    { text: "Memory 详细设计", link: "/zh/unified-memory-model" },
                    { text: "Project / Org 归属决策（待实施）", link: "/zh/project-org-memory-ownership" },
                    { text: "Server", link: "/zh/server" },
                    { text: "本地运行时", link: "/zh/runtime" },
                    { text: "宿主适配", link: "/zh/adapter" },
                    { text: "工作目录绑定", link: "/zh/guides/workspace-binding" },
                    { text: "Memory 界面设计", link: "/zh/macos-memory-ui" },
                    { text: "Review 界面设计", link: "/zh/reviews-ui-design" },
                    { text: "检索与评测", link: "/zh/retrieval-evaluation" },
                    { text: "本地活动记录", link: "/zh/recall" },
                  ]
                },
              ]
            },
            {
              text: "接口参考",
              collapsed: true,
              items: [
                { text: "参考索引", link: "/zh/reference/" },
                { text: "领域接口地图", link: "/zh/reference/domain-api" },
                { text: "MCP：Agent 调用", link: "/zh/mcp" },
                { text: "HTTP 请求与并发", link: "/zh/reference/http-api" },
                { text: "认证与会话", link: "/zh/reference/auth" },
              ]
            },
            {
              text: "开发与维护",
              collapsed: true,
              items: [
                { text: "代码库地图", link: "/zh/repos" },
                { text: "开发流程", link: "/zh/guides/development-workflow" },
                { text: "文档编写约定", link: "/zh/engineering-documents" },
                {
                  text: "性能证据",
                  collapsed: true,
                  items: [
                    { text: "专题索引", link: "/zh/performance/" },
                    { text: "服务端热路径", link: "/zh/performance/server-hot-path" },
                    { text: "macOS 首次就绪", link: "/zh/performance/macos-first-ready" },
                    { text: "延迟模型与诊断", link: "/zh/performance/latency-model" },
                    { text: "gzip 实验", link: "/zh/performance/gzip-experiment" },
                    { text: "签名边界", link: "/zh/performance/signing-boundary" },
                    { text: "验证证据台账", link: "/zh/performance/evidence-ledger" },
                  ]
                },
                {
                  text: "历史文档",
                  collapsed: true,
                  items: [
                    { text: "Project 权威迁移", link: "/zh/project-authority-migration" },
                    { text: "Memory 存储迁移", link: "/zh/guides/rule-store-unification" },
                    { text: "Metaprompt 移除", link: "/zh/meta-prompt" },
                    { text: "已归档 CLI", link: "/zh/guides/cli-commands" },
                    { text: "已归档 TUI", link: "/zh/tui" },
                    { text: "已归档 Attestation", link: "/zh/attestation" },
                  ]
                },
              ]
            },
          ],
          docFooter: {
            prev: "上一页",
            next: "下一页"
          },
          lastUpdated: {
            text: "最后更新"
          },
          darkModeSwitchLabel: "外观",
          lightModeSwitchTitle: "切换到浅色模式",
          darkModeSwitchTitle: "切换到深色模式",
          sidebarMenuLabel: "菜单",
          returnToTopLabel: "返回顶部",
          langMenuLabel: "切换语言",
          skipToContentLabel: "跳到正文"
        }
      },
    },
    markdown: {
      config(md) {
        md.use(footnote);

        md.use(container, "expand", {
          render(tokens, idx) {
            const token = tokens[idx];
            const title = token.info.trim().slice("expand".length).trim() || "More";
            if (token.nesting === 1) {
              return `<details class="vp-expand"><summary>${md.utils.escapeHtml(title)}</summary>\n`;
            }
            return "</details>\n";
          },
        });

        md.use(container, "decision", {
          render(tokens, idx) {
            const token = tokens[idx];
            const title = token.info.trim().slice("decision".length).trim() || "Decision";
            if (token.nesting === 1) {
              return `<div class="custom-block decision"><p class="custom-block-title">${md.utils.escapeHtml(title)}</p>\n`;
            }
            return "</div>\n";
          },
        });

        md.renderer.rules.footnote_ref = (tokens, idx) => {
          const id = tokens[idx].meta.id + 1;
          return `<sup class="footnote-ref"><a href="#fn${id}" id="fnref${id}">${id}</a></sup>`;
        };
      },
    },
    themeConfig: {
      search: {
        provider: "local",
        options: {
          miniSearch: {
            options: {
              // Use the same word boundaries when indexing and querying Chinese prose.
              tokenize(text) {
                return Array.from(new Intl.Segmenter("zh-CN", { granularity: "word" }).segment(text))
                  .filter(({ isWordLike }) => isWordLike)
                  .map(({ segment }) => segment);
              }
            }
          },
          locales: {
            zh: {
              translations: {
                button: {
                  buttonText: "搜索",
                  buttonAriaLabel: "搜索文档"
                },
                modal: {
                  displayDetails: "显示详情",
                  resetButtonTitle: "清除搜索",
                  backButtonTitle: "关闭搜索",
                  noResultsText: "没有找到相关结果",
                  footer: {
                    selectText: "选择",
                    selectKeyAriaLabel: "回车键",
                    navigateText: "切换",
                    navigateUpKeyAriaLabel: "向上箭头",
                    navigateDownKeyAriaLabel: "向下箭头",
                    closeText: "关闭",
                    closeKeyAriaLabel: "退出键"
                  }
                }
              }
            }
          }
        }
      },
      outline: {
        level: [2, 3],
        label: "On this page",
      },
      nav: [
        { text: "Start here", link: "/" },
        { text: "Task guides", link: "/guides/" },
        { text: "Concepts and design", link: "/overview" },
        { text: "Reference", link: "/reference/" },
        { text: "Development", link: "/repos" },
      ],
      sidebar: [
        {
          text: "Start here and quickstart",
          collapsed: false,
          items: [
            { text: "Reading routes", link: "/" },
            { text: "Quickstart introduction", link: "/quickstart/" },
            { text: "Get the App", link: "/quickstart/install" },
            { text: "Connect to your organization", link: "/quickstart/connect" },
            { text: "1. Create a project", link: "/quickstart/create-project" },
            { text: "2. Select Memory", link: "/quickstart/select-memory" },
            { text: "3. Use it with Codex", link: "/quickstart/use-with-agent" },
            { text: "4. Propose an update", link: "/quickstart/update-memory" },
            { text: "5. Review and publish", link: "/quickstart/review-and-publish" },
          ]
        },
        {
          text: "Task guides",
          collapsed: true,
          items: [
            { text: "Find a task", link: "/guides/" },
            { text: "Member task index", link: "/guides/how-to-use-clumsies" },
            { text: "Create a new Memory", link: "/guides/create-memory" },
            { text: "Memory Guidelines", link: "/guides/memory-guidelines" },
            { text: "Connect an agent", link: "/guides/agent-runtime" },
            { text: "Deploy for an organization", link: "/guides/deploy-for-an-org" },
            { text: "Troubleshoot a problem", link: "/guides/troubleshooting" },
            { text: "Connect DeepSeek Harness", link: "/guides/dsh-integration" },
          ]
        },
        {
          text: "Concepts and design",
          collapsed: true,
          items: [
            { text: "Meet Clumsies", link: "/overview" },
            { text: "System architecture", link: "/architecture" },
            { text: "Core data structures", link: "/data-model" },
            { text: "End-to-end flows", link: "/flows" },
            { text: "Glossary", link: "/glossary" },
            {
              text: "Subsystem details",
              collapsed: true,
              items: [
                { text: "Organization Memory", link: "/artifact" },
                { text: "Project selection and binding", link: "/workspace" },
                { text: "Memory design", link: "/unified-memory-model" },
                { text: "Project / Org ownership decision (planned)", link: "/project-org-memory-ownership" },
                { text: "Server", link: "/server" },
                { text: "Local runtime", link: "/runtime" },
                { text: "Host adapters", link: "/adapter" },
                { text: "Workspace binding", link: "/guides/workspace-binding" },
                { text: "Memory UI design", link: "/macos-memory-ui" },
                { text: "Review UI design", link: "/reviews-ui-design" },
                { text: "Retrieval and evaluation", link: "/retrieval-evaluation" },
                { text: "Local activity", link: "/recall" },
              ]
            },
          ]
        },
        {
          text: "Interface reference",
          collapsed: true,
          items: [
            { text: "Reference index", link: "/reference/" },
            { text: "Domain interfaces", link: "/reference/domain-api" },
            { text: "MCP for agents", link: "/mcp" },
            { text: "HTTP requests and concurrency", link: "/reference/http-api" },
            { text: "Authentication and sessions", link: "/reference/auth" },
          ]
        },
        {
          text: "Development and maintenance",
          collapsed: true,
          items: [
            { text: "Codebase map", link: "/repos" },
            { text: "Development workflow", link: "/guides/development-workflow" },
            { text: "Writing documentation", link: "/engineering-documents" },
            {
              text: "Performance evidence",
              collapsed: true,
              items: [
                { text: "Topic index", link: "/performance/" },
                { text: "Server hot path", link: "/performance/server-hot-path" },
                { text: "macOS first readiness", link: "/performance/macos-first-ready" },
                { text: "Latency and diagnosis", link: "/performance/latency-model" },
                { text: "gzip experiment", link: "/performance/gzip-experiment" },
                { text: "Signing boundary", link: "/performance/signing-boundary" },
                { text: "Evidence ledger", link: "/performance/evidence-ledger" },
              ]
            },
            {
              text: "History",
              collapsed: true,
              items: [
                { text: "Project authority migration", link: "/project-authority-migration" },
                { text: "Memory storage migration", link: "/guides/rule-store-unification" },
                { text: "Metaprompt removal", link: "/meta-prompt" },
                { text: "Archived CLI", link: "/guides/cli-commands" },
                { text: "Archived TUI", link: "/tui" },
                { text: "Archived attestation", link: "/attestation" },
              ]
            },
          ]
        },
      ],
      docFooter: {
        prev: "Previous page",
        next: "Next page"
      },
      lastUpdated: {
        text: "Last updated"
      },
      socialLinks: [
        { icon: "github", link: "https://github.com/lilhammerfun/clumsies" }
      ]
    }
  })
);
