namespace Acme.Visibility;

public class Matrix
{
    public void Public() {}
    protected void Protected() {}
    internal void Internal() {}
    private void Private() {}
    protected internal void ProtectedInternal() {}
    private protected void PrivateProtected() {}
}
