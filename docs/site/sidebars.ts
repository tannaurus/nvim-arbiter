import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docsSidebar: [
    'intro',
    {
      type: 'category',
      label: 'Getting Started',
      collapsed: false,
      items: ['installation', 'configuration'],
    },
    {
      type: 'category',
      label: 'Guides',
      collapsed: false,
      items: ['workflow', 'project-rules', 'integrations'],
    },
    {
      type: 'category',
      label: 'Reference',
      items: ['commands', 'keybindings', 'architecture'],
    },
  ],
};

export default sidebars;
