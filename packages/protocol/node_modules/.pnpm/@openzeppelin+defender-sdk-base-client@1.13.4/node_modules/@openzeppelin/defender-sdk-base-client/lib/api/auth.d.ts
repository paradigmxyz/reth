import { CognitoUserSession } from 'amazon-cognito-identity-js';
export type UserPass = {
    Username: string;
    Password: string;
};
export type PoolData = {
    UserPoolId: string;
    ClientId: string;
};
export declare function authenticate(authenticationData: UserPass, poolData: PoolData): Promise<CognitoUserSession>;
export declare function refreshSession(authenticationData: UserPass, poolData: PoolData, session: CognitoUserSession): Promise<CognitoUserSession>;
//# sourceMappingURL=auth.d.ts.map