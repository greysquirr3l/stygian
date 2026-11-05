import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import mermaid from 'astro-mermaid';

// Sidebar order mirrors the old mdBook SUMMARY.md.
export default defineConfig({
  site: 'https://greysquirr3l.github.io',
  base: '/stygian',
  integrations: [
    mermaid({ autoTheme: true }),
    starlight({
      title: 'stygian',
      description: 'High-performance web scraping toolkit for Rust',
      logo: { src: './public/logo.png', alt: 'stygian' },
      favicon: '/favicon.png',
      customCss: ['./src/styles/custom.css'],
      editLink: {
        baseUrl: 'https://github.com/greysquirr3l/stygian/edit/main/site/',
      },
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/greysquirr3l/stygian' },
      ],
      sidebar: [
  {
    "label": "Introduction",
    "slug": "introduction"
  },
  {
    "label": "The Graph Engine",
    "items": [
      {
        "slug": "graph/architecture"
      },
      {
        "slug": "graph/pipelines"
      },
      {
        "slug": "graph/adapters"
      },
      {
        "slug": "graph/production-adapters"
      },
      {
        "slug": "graph/signing"
      },
      {
        "slug": "graph/graphql-plugins"
      },
      {
        "slug": "graph/custom-adapters"
      },
      {
        "slug": "graph/distributed"
      },
      {
        "slug": "graph/escalation"
      },
      {
        "slug": "graph/resilience"
      },
      {
        "slug": "graph/data-sinks"
      },
      {
        "slug": "graph/scrape-exchange"
      },
      {
        "slug": "graph/observability"
      },
      {
        "slug": "graph/performance"
      }
    ]
  },
  {
    "label": "Browser Automation",
    "items": [
      {
        "slug": "browser/overview"
      },
      {
        "slug": "browser/configuration"
      },
      {
        "slug": "browser/stealth"
      },
      {
        "slug": "browser/pool"
      },
      {
        "slug": "browser/page-operations"
      },
      {
        "slug": "browser/dom-query"
      },
      {
        "slug": "browser/extract"
      }
    ]
  },
  {
    "label": "Proxy Rotation",
    "items": [
      {
        "slug": "proxy/overview"
      },
      {
        "slug": "proxy/strategies"
      },
      {
        "slug": "proxy/sticky-sessions"
      },
      {
        "slug": "proxy/health-circuit-breaker"
      },
      {
        "slug": "proxy/integrations"
      }
    ]
  },
  {
    "label": "Charon Diagnostics",
    "items": [
      {
        "slug": "charon/overview"
      },
      {
        "slug": "charon/getting-started"
      },
      {
        "slug": "charon/slo-policy"
      },
      {
        "slug": "charon/operations"
      }
    ]
  },
  {
    "label": "Reference",
    "items": [
      {
        "slug": "reference/charon-release"
      },
      {
        "slug": "reference/backwards-compatibility"
      },
      {
        "slug": "reference/env-vars"
      },
      {
        "slug": "reference/security-policy"
      },
      {
        "slug": "reference/testing"
      }
    ]
  },
  {
    "label": "MCP \u2014 Model Context Protocol",
    "items": [
      {
        "slug": "mcp/overview"
      },
      {
        "slug": "mcp/graph-tools"
      },
      {
        "slug": "mcp/browser-tools"
      },
      {
        "slug": "mcp/proxy-tools"
      },
      {
        "slug": "mcp/plugin-tools"
      },
      {
        "slug": "mcp/plugin-persistence-pattern"
      },
      {
        "slug": "mcp/aggregator"
      }
    ]
  }
],
    }),
  ],
});
