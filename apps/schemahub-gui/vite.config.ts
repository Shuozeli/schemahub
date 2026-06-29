import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

const tailscaleHost = process.env.TAILSCALE_HOST;

export default defineConfig({
  plugins: [react()],
  server: {
    host: process.env.TAILSCALE_IP || '0.0.0.0',
    port: 5173,
    allowedHosts: tailscaleHost ? [tailscaleHost] : [],
  },
});
