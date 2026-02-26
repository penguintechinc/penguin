import { useState, useEffect } from 'react';
import { useAuth } from '../hooks/useAuth';
import { helloApi, goApi } from '../hooks/useApi';
import Card from '../components/Card';
import TabNavigation from '../components/TabNavigation';

export default function Dashboard() {
  const { user } = useAuth();
  const [activeTab, setActiveTab] = useState('overview');
  const [helloMessage, setHelloMessage] = useState<string | null>(null);
  const [goStatus, setGoStatus] = useState<Record<string, unknown> | null>(null);
  const [isLoading, setIsLoading] = useState(true);

  const tabs = [
    { id: 'overview', label: 'Overview' },
    { id: 'status', label: 'System Status' },
    { id: 'metrics', label: 'Metrics' },
  ];

  useEffect(() => {
    const fetchData = async () => {
      setIsLoading(true);
      try {
        // Fetch hello message from Flask backend
        const hello = await helloApi.getProtected();
        setHelloMessage(hello.message);

        // Try to fetch Go backend status
        try {
          const status = await goApi.status();
          setGoStatus(status);
        } catch {
          // Go backend might not be running
          setGoStatus(null);
        }
      } catch (err) {
        console.error('Failed to fetch data:', err);
      } finally {
        setIsLoading(false);
      }
    };

    fetchData();
  }, []);

  return (
    <div>
      {/* Header */}
      <div className="mb-6">
        <h1 className="text-2xl font-bold text-gold-400">Dashboard</h1>
        <p className="text-dark-400 mt-1">
          Welcome back, {user?.full_name || 'User'}
        </p>
      </div>

      {/* Tab Navigation */}
      <TabNavigation tabs={tabs} activeTab={activeTab} onChange={setActiveTab} />

      {/* Tab Content */}
      <div className="mt-6">
        {activeTab === 'overview' && (
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
            {/* Welcome Card */}
            <Card title="Welcome">
              {isLoading ? (
                <div className="animate-pulse h-12 bg-dark-700 rounded"></div>
              ) : (
                <p className="text-dark-300">{helloMessage || 'Hello from WebUI!'}</p>
              )}
            </Card>

            {/* User Info Card */}
            <Card title="Your Account">
              <div className="space-y-2">
                <div>
                  <span className="text-dark-400 text-sm">Email:</span>
                  <p className="text-gold-400">{user?.email}</p>
                </div>
                <div>
                  <span className="text-dark-400 text-sm">Role:</span>
                  <p>
                    <span className={`badge badge-${user?.role}`}>{user?.role}</span>
                  </p>
                </div>
              </div>
            </Card>

            {/* Quick Stats Card */}
            <Card title="Quick Stats">
              <div className="grid grid-cols-2 gap-4">
                <div>
                  <div className="text-2xl font-bold text-gold-400">3</div>
                  <div className="text-sm text-dark-400">Services</div>
                </div>
                <div>
                  <div className="text-2xl font-bold text-green-400">Active</div>
                  <div className="text-sm text-dark-400">Status</div>
                </div>
              </div>
            </Card>
          </div>
        )}

        {activeTab === 'status' && (
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
            {/* Flask Backend Status */}
            <Card title="Flask Backend">
              <div className="space-y-3">
                <div className="flex items-center justify-between">
                  <span className="text-dark-400">Status</span>
                  <span className="text-green-400">● Connected</span>
                </div>
                <div className="flex items-center justify-between">
                  <span className="text-dark-400">Endpoint</span>
                  <span className="text-gold-400">/api/v1</span>
                </div>
                <div className="flex items-center justify-between">
                  <span className="text-dark-400">Features</span>
                  <span className="text-dark-300">Auth, Users, Hello</span>
                </div>
              </div>
            </Card>

            {/* Go Backend Status */}
            <Card title="Go Backend">
              {goStatus ? (
                <div className="space-y-3">
                  <div className="flex items-center justify-between">
                    <span className="text-dark-400">Status</span>
                    <span className="text-green-400">● Connected</span>
                  </div>
                  <div className="flex items-center justify-between">
                    <span className="text-dark-400">Version</span>
                    <span className="text-gold-400">{(goStatus as Record<string, string>).version || 'N/A'}</span>
                  </div>
                  <div className="flex items-center justify-between">
                    <span className="text-dark-400">Go Version</span>
                    <span className="text-dark-300">{(goStatus as Record<string, string>).go_version || 'N/A'}</span>
                  </div>
                </div>
              ) : (
                <div className="text-dark-400">
                  <span className="text-yellow-400">● Not Connected</span>
                  <p className="text-sm mt-2">Go backend is not available</p>
                </div>
              )}
            </Card>
          </div>
        )}

        {activeTab === 'metrics' && (
          <Card title="System Metrics">
            <p className="text-dark-400">
              Metrics visualization will be available when Prometheus/Grafana are configured.
            </p>
            <div className="mt-4 p-4 bg-dark-900 rounded-lg">
              <pre className="text-sm text-dark-300 font-mono">
                {goStatus ? JSON.stringify(goStatus, null, 2) : 'No metrics available'}
              </pre>
            </div>
          </Card>
        )}
      </div>
    </div>
  );
}
