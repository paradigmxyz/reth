import { Method, AxiosError } from 'axios';
type PickedAxiosErrorFields<TError> = {
    request: {
        path: string;
        method: Method;
    };
    response: {
        status: number;
        statusText: string;
        data?: TError;
    };
};
export declare class DefenderApiResponseError<TErrorData = unknown> extends Error {
    name: string;
    request: PickedAxiosErrorFields<TErrorData>['request'];
    response: PickedAxiosErrorFields<TErrorData>['response'];
    constructor(axiosError: AxiosError<TErrorData>);
}
export {};
//# sourceMappingURL=api-error.d.ts.map