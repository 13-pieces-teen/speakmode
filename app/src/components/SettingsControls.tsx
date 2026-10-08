import { useId, type ReactNode } from 'react';

export function SettingsSection({ title, icon, className = '', children }: {
  title: string; icon: ReactNode; className?: string; children: ReactNode;
}) {
  const titleId = useId();
  return <section className={`settings-section ${className}`} aria-labelledby={titleId}>
      <header className="settings-section-heading">
        <span className="settings-section-icon" aria-hidden="true">{icon}</span>
        <h2 id={titleId}>{title}</h2>
      </header>
      {children}
    </section>;
}

export function SettingRow({ title, description, htmlFor, children, className = '' }: {
  title: string; description: string; htmlFor?: string; children: ReactNode; className?: string;
}) {
  return <div className={`setting-row ${className}`}>
    <div className="setting-row-copy">
      {htmlFor ? <label htmlFor={htmlFor}>{title}</label> : <h3>{title}</h3>}
      <p id={htmlFor ? `${htmlFor}-description` : undefined}>{description}</p>
    </div>
    <div className="setting-row-control">{children}</div>
  </div>;
}
