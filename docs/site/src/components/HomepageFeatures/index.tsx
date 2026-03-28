import type {ReactNode} from 'react';
import Heading from '@theme/Heading';
import styles from './styles.module.css';

type FeatureItem = {
  title: string;
  description: ReactNode;
};

const FeatureList: FeatureItem[] = [
  {
    title: 'PR-style diffs',
    description: (
      <>
        Dedicated review tabpage with a file panel and diff viewer. Diff against
        a branch or review unstaged working tree changes.
      </>
    ),
  },
  {
    title: 'Line-anchored threads',
    description: (
      <>
        Comment on any diff line and get a streaming response. Each thread is a
        scoped conversation anchored to a specific line, just like a PR review.
      </>
    ),
  },
  {
    title: 'Review memory',
    description: (
      <>
        Conventions you enforce in one thread get extracted and fed into every
        subsequent prompt. The agent learns your preferences as you review.
      </>
    ),
  },
  {
    title: 'Project rules',
    description: (
      <>
        Persistent, file-aware instructions loaded from markdown files with TOML
        frontmatter. Scope rules to file types and scenarios.
      </>
    ),
  },
  {
    title: 'Agent self-review',
    description: (
      <>
        The agent reviews its own diff and flags concerns before you start.
        Apply all self-review feedback in one step.
      </>
    ),
  },
  {
    title: 'Built in Rust',
    description: (
      <>
        Native performance via{' '}
        <a href="https://github.com/noib3/nvim-oxi">nvim-oxi</a>. Works with
        Cursor CLI and Claude Code CLI.
      </>
    ),
  },
];

function Feature({title, description}: FeatureItem) {
  return (
    <div className="col col--4" style={{marginBottom: '1.5rem'}}>
      <div className="feature-card">
        <Heading as="h3">{title}</Heading>
        <p>{description}</p>
      </div>
    </div>
  );
}

export default function HomepageFeatures(): ReactNode {
  return (
    <section className={styles.features}>
      <div className="container">
        <div className="row">
          {FeatureList.map((props, idx) => (
            <Feature key={idx} {...props} />
          ))}
        </div>
      </div>
    </section>
  );
}
