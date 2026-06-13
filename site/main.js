import { mountApp } from "@opentf/web";

// 1. Discover Pages & Layouts (including 404)
const pages = import.meta.glob('./app/**/{page,layout,404}.{jsx,tsx}', { eager: true });

// 2. Discover Route Guard
const guards = import.meta.glob('./app/routeGuard.{js,ts,jsx,tsx}', { eager: true });
const guard = Object.values(guards)[0]?.default;

// 3. Bootstrap the Web App Framework Application
mountApp({ 
  pages, 
  guard 
});
