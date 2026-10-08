import { Card } from '@radix-ui/themes';
import type { ReactNode } from 'react';
import './HomeSectionCard.css';

interface HomeSectionCardProps {
  titleId: string;
  title: string;
  action?: ReactNode;
  className?: string;
  children: ReactNode;
}

/** Shared outer grouping for the four peer sections on Home. */
export function HomeSectionCard({ titleId, title, action, className = '', children }: HomeSectionCardProps) {
  return <Card asChild size="3" variant="surface" className={`home-section-card ${className}`}>
    <section aria-labelledby={titleId}>
      <header className="section-heading home-section-heading">
        <h2 id={titleId} className="section-title">{title}</h2>
        {action && <div className="home-section-action">{action}</div>}
      </header>
      {children}
    </section>
  </Card>;
}
