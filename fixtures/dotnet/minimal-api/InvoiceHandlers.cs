using Acme.Invoicing.MinimalApi.Data;

namespace Acme.Invoicing.MinimalApi.Handlers;

public static class InvoiceHandlers
{
    public static Invoice GetInvoice(IInvoiceRepository repo, int id) => repo.Find(id);
}
