using Acme.Invoicing.Data;

namespace Acme.Invoicing.Api;

// Call-driven recognition (PR6): DI registration linking the interface to
// its implementation, plus the `AppDbContext` registration.
public class Startup
{
    public void ConfigureServices(IServiceCollection services)
    {
        services.AddScoped<IInvoiceRepository, InvoiceRepository>();
        services.AddDbContext<AppDbContext>();
        services.AddControllers();
    }
}
