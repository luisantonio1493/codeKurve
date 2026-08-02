// providers/imports arrays plus an HTTP_INTERCEPTORS registration
// (task 4.7).
@NgModule({
  imports: [SharedModule],
  providers: [
    InvoiceApiRepository,
    { provide: HTTP_INTERCEPTORS, useClass: AuthInterceptor, multi: true },
  ],
})
export class AppModule {}
