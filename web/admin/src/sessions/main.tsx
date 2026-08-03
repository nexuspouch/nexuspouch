import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { FeedbackProvider } from '../shared/ui/feedback';
import { App } from './App';
import './styles.css';

const root = document.getElementById('root');
if (!root) throw new Error('missing #root');

createRoot(root).render(
  <StrictMode>
    <FeedbackProvider>
      <App />
    </FeedbackProvider>
  </StrictMode>,
);
