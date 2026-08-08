import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

const tailscaleHost = process.env.TAILSCALE_HOST;
const tailscaleBind = process.env.TAILSCALE_IP || '0.0.0.0';
const allowedHosts = tailscaleHost ? [tailscaleHost] : [];

export default defineConfig({
  plugins: [react()],
  server: {
    host: tailscaleBind,
    port: 5173,
    allowedHosts,
  },
  preview: {
    host: tailscaleBind,
    port: 4173,
    allowedHosts,
  },
});
