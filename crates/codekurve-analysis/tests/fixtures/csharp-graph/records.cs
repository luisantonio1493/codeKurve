namespace Acme.Models;
public record Invoice(string Id);
public record struct InvoiceId(string Value);
public enum State { Open, Closed }
