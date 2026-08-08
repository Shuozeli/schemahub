import type { NextConfig } from "next";

const allowedDevOrigins = process.env.TAILSCALE_HOST
  ? [process.env.TAILSCALE_HOST]
  : undefined;

const nextConfig: NextConfig = {
  allowedDevOrigins,
  output: "export",
  reactStrictMode: true,
};

export default nextConfig;
