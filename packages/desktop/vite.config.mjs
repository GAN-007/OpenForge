import { defineConfig } from 'vite';

// The local launcher uses this same-origin proxy when default ports are occupied.
export default defineConfig({
  server: {
    host: '127.0.0.1',
    proxy: {
      '/openforge-daemon': {
        target: process.env.OPENFORGE_DAEMON_URL || 'http://127.0.0.1:8765',
        rewrite: (path) => path.replace(/^\/openforge-daemon/, ''),
      },
    },
  },
});
