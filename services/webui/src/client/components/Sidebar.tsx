import { Link, useLocation } from 'react-router-dom';
import { useAuth } from '../hooks/useAuth';
import type { NavCategory, UserRole } from '../types';

interface SidebarProps {
  collapsed: boolean;
  onToggle: () => void;
}

// Navigation structure - Elder pattern with categories
const navigation: NavCategory[] = [
  {
    label: 'Main',
    items: [
      { label: 'Dashboard', path: '/', icon: '📊' },
      { label: 'Profile', path: '/profile', icon: '👤' },
    ],
  },
  {
    label: 'Management',
    roles: ['admin', 'maintainer'],
    items: [
      { label: 'Settings', path: '/settings', icon: '⚙️', roles: ['admin', 'maintainer'] },
    ],
  },
  {
    label: 'Administration',
    roles: ['admin'],
    items: [
      { label: 'Users', path: '/users', icon: '👥', roles: ['admin'] },
    ],
  },
];

export default function Sidebar({ collapsed, onToggle }: SidebarProps) {
  const location = useLocation();
  const { user, logout, hasRole } = useAuth();

  // Filter navigation based on user role
  const filteredNav = navigation
    .filter((category) => !category.roles || hasRole(category.roles as UserRole[]))
    .map((category) => ({
      ...category,
      items: category.items.filter((item) => !item.roles || hasRole(item.roles as UserRole[])),
    }))
    .filter((category) => category.items.length > 0);

  const isActive = (path: string) => {
    if (path === '/') {
      return location.pathname === '/';
    }
    return location.pathname.startsWith(path);
  };

  return (
    <aside
      className={`sidebar ${collapsed ? 'sidebar-collapsed' : 'sidebar-expanded'}`}
    >
      {/* Header */}
      <div className="flex items-center justify-between h-16 px-4 border-b border-dark-700">
        {!collapsed && (
          <span className="text-xl font-bold text-gold-gradient">WebUI</span>
        )}
        <button
          onClick={onToggle}
          className="p-2 rounded-lg hover:bg-dark-800 text-gold-400"
          title={collapsed ? 'Expand' : 'Collapse'}
        >
          {collapsed ? '→' : '←'}
        </button>
      </div>

      {/* Navigation */}
      <nav className="flex-1 overflow-y-auto py-4">
        {filteredNav.map((category) => (
          <div key={category.label} className="mb-4">
            {!collapsed && (
              <div className="sidebar-category">{category.label}</div>
            )}
            {category.items.map((item) => (
              <Link
                key={item.path}
                to={item.path}
                className={`sidebar-item ${isActive(item.path) ? 'sidebar-item-active' : ''}`}
                title={collapsed ? item.label : undefined}
              >
                <span className="text-lg">{item.icon}</span>
                {!collapsed && <span className="ml-3">{item.label}</span>}
              </Link>
            ))}
          </div>
        ))}
      </nav>

      {/* User section */}
      <div className="border-t border-dark-700 p-4">
        {!collapsed && user && (
          <div className="mb-3">
            <div className="text-sm text-gold-400 truncate">{user.full_name}</div>
            <div className="text-xs text-dark-400 truncate">{user.email}</div>
            <span className={`badge mt-1 badge-${user.role}`}>{user.role}</span>
          </div>
        )}
        <button
          onClick={() => logout()}
          className={`w-full flex items-center ${
            collapsed ? 'justify-center' : ''
          } px-4 py-2 text-sm text-red-400 hover:bg-dark-800 rounded-lg`}
          title="Logout"
        >
          <span>🚪</span>
          {!collapsed && <span className="ml-2">Logout</span>}
        </button>
      </div>
    </aside>
  );
}
