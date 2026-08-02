using Acme.Invoicing.Data;

namespace Acme.Invoicing.Data;

// EF Core exception (D11): the one `DbSet<T>` member of a `DbContext`
// subclass -> `PersistsTo`.
public class AppDbContext : DbContext
{
    public DbSet<Invoice> Invoices { get; set; }
}
