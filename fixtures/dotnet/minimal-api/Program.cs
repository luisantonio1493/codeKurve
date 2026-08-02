using Acme.Invoicing.MinimalApi.Data;
using static Acme.Invoicing.MinimalApi.Handlers.InvoiceHandlers;

// Call-driven recognition (PR6): minimal-API route + DI registration, proving
// the same shape the controller variant covers via attributes.
var builder = WebApplication.CreateBuilder(args);
builder.Services.AddScoped<IInvoiceRepository, InvoiceRepository>();
builder.Services.AddDbContext<AppDbContext>();
builder.Services.AddOpenApi();

var app = builder.Build();
app.MapGet("/invoices/{id}", GetInvoice);
app.Run();
