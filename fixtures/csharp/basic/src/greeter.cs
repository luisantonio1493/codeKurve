using Acme.Contracts;
namespace Acme.App;
public class Greeter : IGreeter
{
    public string Greet(string name) => $"Hello {name}";
}
