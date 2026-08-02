// Referenced only from `app.routes.ts`'s `canActivate` array — no decorator
// needed for the guard itself, since `RegisteredAs` fires off the routes
// array, not off this class.
export class AuthGuard {
  canActivate(): boolean {
    return true;
  }
}
