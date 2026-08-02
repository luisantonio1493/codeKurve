using Acme.Invoicing.MinimalApi.Data;

namespace Acme.Invoicing.MinimalApi.Data;

public class AppDbContext : DbContext
{
    public DbSet<Invoice> Invoices { get; set; }
}
