import {themes as prismThemes} from 'prism-react-renderer';
import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';

const config: Config = {
  title: 'arbiter',
  tagline: 'Review workbench for Neovim. PR-style diffs, line-anchored threads, and a structured feedback loop with AI coding agents.',
  favicon: 'img/favicon.ico',

  future: {
    v4: true,
  },

  url: 'https://tannaurus.github.io',
  baseUrl: '/nvim-arbiter/',

  organizationName: 'tannaurus',
  projectName: 'nvim-arbiter',
  trailingSlash: false,

  onBrokenLinks: 'throw',

  markdown: {
    hooks: {
      onBrokenMarkdownLinks: 'warn',
    },
  },

  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      {
        docs: {
          sidebarPath: './sidebars.ts',
          editUrl:
            'https://github.com/tannaurus/nvim-arbiter/tree/main/docs/site/',
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    colorMode: {
      defaultMode: 'dark',
      respectPrefersColorScheme: true,
    },
    navbar: {
      title: 'arbiter',
      items: [
        {
          type: 'docSidebar',
          sidebarId: 'docsSidebar',
          position: 'left',
          label: 'Docs',
        },
        {
          href: 'https://github.com/tannaurus/nvim-arbiter',
          label: 'GitHub',
          position: 'right',
        },
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'Docs',
          items: [
            {label: 'Getting Started', to: '/docs/installation'},
            {label: 'Workflow', to: '/docs/workflow'},
            {label: 'Configuration', to: '/docs/configuration'},
          ],
        },
        {
          title: 'More',
          items: [
            {
              label: 'GitHub',
              href: 'https://github.com/tannaurus/nvim-arbiter',
            },
            {
              label: 'Releases',
              href: 'https://github.com/tannaurus/nvim-arbiter/releases',
            },
          ],
        },
      ],
      copyright: `Copyright © ${new Date().getFullYear()} arbiter. Built with Docusaurus.`,
    },
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.dracula,
      additionalLanguages: ['lua', 'bash', 'toml', 'json', 'rust', 'vim'],
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
