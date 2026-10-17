import React from 'react';
import ReactDOM from 'react-dom/client';
import { Theme } from '@radix-ui/themes';
import '@radix-ui/themes/styles.css';
import { App } from './App';
import './styles.css';
import './lucian.css';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <Theme appearance="light" accentColor="blue" grayColor="slate" radius="large" scaling="100%">
      <App />
    </Theme>
  </React.StrictMode>,
);
