import { defineConfig } from "vitepress";
import footnote from "markdown-it-footnote";
import container from "markdown-it-container";
import { withMermaid } from "vitepress-plugin-mermaid";

export default withMermaid(
  defineConfig({
    lang: "en-US",
    title: "clumsies",
    description: "Understand Clumsies: architecture, data structures, interfaces, and practical guides for shared agent memory.",
    base: "/",
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
            { text: "架构", link: "/zh/architecture" },
            { text: "数据", link: "/zh/data-model" },
            { text: "接口", link: "/zh/reference/domain-api" },
            { text: "指南", link: "/zh/guides/" },
          ],
          sidebar: [
            {
              text: "从这里开始",
              collapsed: false,
              items: [
                { text: "阅读路线", link: "/zh/" },
                { text: "认识 Clumsies", link: "/zh/overview" },
                { text: "系统架构", link: "/zh/architecture" },
                { text: "核心数据结构", link: "/zh/data-model" },
                { text: "完整流程", link: "/zh/flows" },
              ]
            },
            {
              text: "接口参考",
              collapsed: false,
              items: [
                { text: "领域接口地图", link: "/zh/reference/domain-api" },
                { text: "MCP：Agent 如何调用", link: "/zh/mcp" },
                { text: "HTTP 请求与并发", link: "/zh/reference/http-api" },
                { text: "认证与会话", link: "/zh/reference/auth" },
                { text: "术语表", link: "/zh/glossary" },
              ]
            },
            {
              text: "使用与运维",
              collapsed: false,
              items: [
                { text: "指南首页", link: "/zh/guides/" },
                { text: "第一次使用", link: "/zh/guides/how-to-use-clumsies" },
                { text: "Agent 接入", link: "/zh/guides/agent-runtime" },
                { text: "组织部署", link: "/zh/guides/deploy-for-an-org" },
                { text: "排查问题", link: "/zh/guides/troubleshooting" },
                { text: "本地开发", link: "/zh/guides/development-workflow" },
              ]
            },
            {
              text: "深入设计",
              collapsed: true,
              items: [
                { text: "Organization Memory", link: "/zh/artifact" },
                { text: "Project 选择与绑定", link: "/zh/workspace" },
                { text: "Memory 详细设计", link: "/zh/unified-memory-model" },
                { text: "Server", link: "/zh/server" },
                { text: "本地运行时", link: "/zh/runtime" },
                { text: "宿主适配", link: "/zh/adapter" },
                { text: "AgentRun 生命周期", link: "/zh/guides/agent-run-injection" },
                { text: "Memory 界面", link: "/zh/macos-memory-ui" },
                { text: "Review 界面", link: "/zh/reviews-ui-design" },
                { text: "检索与评测", link: "/zh/retrieval-evaluation" },
                { text: "本地活动记录", link: "/zh/recall" },
                { text: "代码库地图", link: "/zh/repos" },
                { text: "DSH 集成", link: "/zh/guides/dsh-integration" },
              ]
            },
            {
              text: "性能与验证",
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
              text: "维护与历史",
              collapsed: true,
              items: [
                { text: "文档编写约定", link: "/zh/engineering-documents" },
                { text: "参考资料索引", link: "/zh/reference/" },
                { text: "Project 权威迁移", link: "/zh/project-authority-migration" },
                { text: "Memory 存储迁移", link: "/zh/guides/rule-store-unification" },
                { text: "Metaprompt 移除", link: "/zh/meta-prompt" },
                { text: "已归档 CLI", link: "/zh/guides/cli-commands" },
                { text: "已归档 TUI", link: "/zh/tui" },
                { text: "已归档 Attestation", link: "/zh/attestation" },
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
        { text: "Architecture", link: "/architecture" },
        { text: "Data", link: "/data-model" },
        { text: "Interfaces", link: "/reference/domain-api" },
        { text: "Guides", link: "/guides/" },
      ],
      sidebar: [
        {
          text: "Start here",
          collapsed: false,
          items: [
            { text: "Reading paths", link: "/" },
            { text: "Meet Clumsies", link: "/overview" },
            { text: "System architecture", link: "/architecture" },
            { text: "Core data structures", link: "/data-model" },
            { text: "End-to-end flows", link: "/flows" },
          ]
        },
        {
          text: "Interface reference",
          collapsed: false,
          items: [
            { text: "Domain interfaces", link: "/reference/domain-api" },
            { text: "MCP for agents", link: "/mcp" },
            { text: "HTTP requests and concurrency", link: "/reference/http-api" },
            { text: "Authentication and sessions", link: "/reference/auth" },
            { text: "Glossary", link: "/glossary" },
          ]
        },
        {
          text: "Guides and operations",
          collapsed: false,
          items: [
            { text: "Guide index", link: "/guides/" },
            { text: "First use", link: "/guides/how-to-use-clumsies" },
            { text: "Agent integration", link: "/guides/agent-runtime" },
            { text: "Deploy for an organization", link: "/guides/deploy-for-an-org" },
            { text: "Troubleshooting", link: "/guides/troubleshooting" },
            { text: "Local development", link: "/guides/development-workflow" },
          ]
        },
        {
          text: "Design details",
          collapsed: true,
          items: [
            { text: "Organization Memory", link: "/artifact" },
            { text: "Project selection and binding", link: "/workspace" },
            { text: "Memory design", link: "/unified-memory-model" },
            { text: "Server", link: "/server" },
            { text: "Local runtime", link: "/runtime" },
            { text: "Host adapters", link: "/adapter" },
            { text: "AgentRun lifecycle", link: "/guides/agent-run-injection" },
            { text: "Memory UI", link: "/macos-memory-ui" },
            { text: "Review UI", link: "/reviews-ui-design" },
            { text: "Retrieval and evaluation", link: "/retrieval-evaluation" },
            { text: "Local activity", link: "/recall" },
            { text: "Codebase map", link: "/repos" },
            { text: "DSH integration", link: "/guides/dsh-integration" },
          ]
        },
        {
          text: "Performance and validation",
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
          text: "Maintenance and history",
          collapsed: true,
          items: [
            { text: "Writing documentation", link: "/engineering-documents" },
            { text: "Reference index", link: "/reference/" },
            { text: "Project authority migration", link: "/project-authority-migration" },
            { text: "Memory storage migration", link: "/guides/rule-store-unification" },
            { text: "Metaprompt removal", link: "/meta-prompt" },
            { text: "Archived CLI", link: "/guides/cli-commands" },
            { text: "Archived TUI", link: "/tui" },
            { text: "Archived attestation", link: "/attestation" },
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
