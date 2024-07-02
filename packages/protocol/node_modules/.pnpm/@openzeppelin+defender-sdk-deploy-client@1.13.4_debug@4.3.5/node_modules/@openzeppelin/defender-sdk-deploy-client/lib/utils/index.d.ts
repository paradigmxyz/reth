import { AxiosInstance } from 'axios';
export type TestClient<T> = Omit<T, 'api' | 'apiKey' | 'apiSecret'> & {
    api: AxiosInstance;
    apiKey: string;
    apiSecret: string;
    init: () => Promise<void>;
};
//# sourceMappingURL=index.d.ts.map