using Acme.Contracts;

namespace Acme.Billing;

[Service]
public partial class InvoiceProcessor : InvoiceBase, IInvoiceProcessor
{
    public decimal Process(decimal amount)
    {
        var formatter = new Formatter();
        return new InvoiceBase().Calculate(formatter.Format(amount));
    }
}

public class Formatter
{
    public decimal Format(decimal amount) => amount;
}
