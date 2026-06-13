/**
 * Global Route Guard
 * 
 * Use this to protect routes, redirect users, or handle global navigation logic.
 */
export default async function routeGuard(to, { next, redirect }) {
  // Add your auth logic here
  // e.g., if (!isLoggedIn) return redirect('/login')
  
  next();
}
