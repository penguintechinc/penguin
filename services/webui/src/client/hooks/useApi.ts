import { useState, useCallback } from 'react';
import api from '../lib/api';
import type { User, CreateUserData, UpdateUserData, PaginatedResponse } from '../types';

// Generic API hook for loading states
export function useApiCall<T>() {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);

  const execute = useCallback(async (apiCall: () => Promise<T>) => {
    setIsLoading(true);
    setError(null);
    try {
      const result = await apiCall();
      setData(result);
      return result;
    } catch (err) {
      const message = err instanceof Error ? err.message : 'An error occurred';
      setError(message);
      throw err;
    } finally {
      setIsLoading(false);
    }
  }, []);

  return { data, error, isLoading, execute, setData };
}

// Users API
export const usersApi = {
  list: async (page = 1, perPage = 20): Promise<PaginatedResponse<User>> => {
    const response = await api.get('/users', { params: { page, per_page: perPage } });
    return response.data;
  },

  get: async (id: number): Promise<User> => {
    const response = await api.get(`/users/${id}`);
    return response.data;
  },

  create: async (data: CreateUserData): Promise<User> => {
    const response = await api.post('/users', data);
    return response.data;
  },

  update: async (id: number, data: UpdateUserData): Promise<User> => {
    const response = await api.put(`/users/${id}`, data);
    return response.data;
  },

  delete: async (id: number): Promise<void> => {
    await api.delete(`/users/${id}`);
  },
};

// Hello world API (example)
export const helloApi = {
  get: async (): Promise<{ message: string; timestamp: string }> => {
    const response = await api.get('/hello');
    return response.data;
  },

  getProtected: async (): Promise<{ message: string; user: string; role: string }> => {
    const response = await api.get('/hello/protected');
    return response.data;
  },
};

// Go backend API (high-performance endpoints)
export const goApi = {
  status: async (): Promise<Record<string, unknown>> => {
    const response = await api.get('/go/status');
    return response.data;
  },

  numaInfo: async (): Promise<Record<string, unknown>> => {
    const response = await api.get('/go/numa/info');
    return response.data;
  },

  memoryStats: async (): Promise<Record<string, unknown>> => {
    const response = await api.get('/go/memory/stats');
    return response.data;
  },
};
